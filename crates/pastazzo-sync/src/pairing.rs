use crate::remote::Remote;
use crate::state::{KeyStorage, State};
use crate::{Result, now};
use pastazzo_core::Id;
use pastazzo_core::api::{B64, PairingCreate, PairingPeer, PairingReply, PairingStatus};
use pastazzo_core::approval::approval_code;
use pastazzo_core::device::{DeviceKeys, DevicePublic, SealedDeviceRecord};
use pastazzo_core::invite::{Invite, InviteKey, InviteVerifier};
use pastazzo_core::pairing::{self, PairingLink};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize)]
pub struct Offer {
    pub id: Id,
    pub verifier: [u8; 32],
    pub expires_at: u64,
}

impl Offer {
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let directory = path.parent().ok_or("missing pairing directory")?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|e| e.to_string())?;
        let temporary = path.with_extension("tmp");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(self).map_err(|e| e.to_string())?)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(temporary, path).map_err(|e| e.to_string())
    }

    pub fn load(path: &std::path::Path) -> Result<Self> {
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
}

pub fn create(state: &State) -> Result<(PairingLink, Offer)> {
    if state.account_secret.is_none() {
        return Err("this Mac cannot approve devices".into());
    }
    let key = InviteKey::generate(&mut OsRng);
    let verifier = key.verifier();
    let status = Remote::new(&state.server_url).pairing_create(
        state,
        &PairingCreate {
            id: B64(key.id.to_vec()),
            verifier: B64(verifier.public.to_vec()),
            username: state.username.clone(),
        },
    )?;
    let offer = Offer {
        id: key.id,
        verifier: verifier.public,
        expires_at: status.expires_at,
    };
    let link = PairingLink {
        invite: Invite {
            server_url: state.server_url.clone(),
            fingerprint: state.identity.fingerprint(),
            key,
        },
        account: state.account,
        owner: state.device.public(),
        username: state.username.clone(),
        expires_at: status.expires_at,
    };
    Ok((link, offer))
}

pub fn status(state: &State, offer: &Offer) -> Result<PairingStatus> {
    let status = Remote::new(&state.server_url).pairing_status(state, &offer.id)?;
    if let Some(peer) = &status.peer {
        validate_peer(state, offer, peer)?;
    }
    Ok(status)
}

fn validate_peer(state: &State, offer: &Offer, peer: &PairingPeer) -> Result<DevicePublic> {
    let device = DevicePublic::from_bytes(&peer.device.0).map_err(|e| e.to_string())?;
    pairing::verify_peer(
        &InviteVerifier {
            id: offer.id,
            public: offer.verifier,
        },
        &state.username,
        &device,
        &peer.name,
        &peer.proof.0,
    )
    .map_err(|_| "the QR request could not be verified")?;
    Ok(device)
}

pub fn approve(state: &State, offer: &Offer, expected_code: &str) -> Result<()> {
    if offer.expires_at <= now() {
        return Err("QR expired; show a new QR".into());
    }
    let status = status(state, offer)?;
    let peer = status.peer.ok_or("no iPhone has scanned this QR")?;
    let device = validate_peer(state, offer, &peer)?;
    if approval_code(&device) != expected_code {
        return Err("the pairing request changed; scan a new QR".into());
    }
    let grant = pairing::seal_grant(
        &state.device,
        &offer.id,
        &state.account,
        &device,
        &state.account_key,
        state
            .account_secret
            .as_ref()
            .ok_or("this Mac cannot approve devices")?,
        &mut OsRng,
    );
    Remote::new(&state.server_url).pairing_grant(state, &offer.id, &grant)
}

pub fn join(link: &str, name: &str, on_code: &mut dyn FnMut(&str)) -> Result<State> {
    let link = PairingLink::parse(link).map_err(|_| "this is not a Pastazzo pairing QR")?;
    if link.expires_at <= now() {
        return Err("QR expired; show a new QR on your Mac".into());
    }
    let remote = Remote::new(&link.invite.server_url);
    let identity = crate::account::pinned_identity(&remote, &link.invite.fingerprint)?;
    let device = DeviceKeys::generate(&mut OsRng);
    let peer = PairingPeer {
        device: B64(device.public().to_bytes().to_vec()),
        name: name.into(),
        proof: B64(link
            .invite
            .key
            .start_proof(
                &link.username,
                &pairing::peer_message(&device.public(), name),
            )
            .to_vec()),
    };
    pairing::verify_peer(
        &link.invite.key.verifier(),
        &link.username,
        &device.public(),
        name,
        &peer.proof.0,
    )
    .map_err(|e| e.to_string())?;
    on_code(&approval_code(&device.public()));
    let (account_key, account_secret) = loop {
        if link.expires_at <= now() {
            return Err("QR expired; show a new QR on your Mac".into());
        }
        let reply: PairingReply = remote.post_json(
            &format!("/v1/pairings/{}/request", B64::encode(&link.invite.key.id)),
            &peer,
        )?;
        if reply.account.array::<16>() != Some(link.account) {
            return Err("the pairing account was changed".into());
        }
        if let Some(grant) = reply.grant {
            break pairing::open_grant(&link, &device, &grant.0)
                .map_err(|_| "the pairing approval could not be verified")?;
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    let mut state = State {
        server_url: link.invite.server_url,
        identity,
        username: link.username,
        account: link.account,
        account_key,
        account_secret: Some(account_secret),
        device,
        device_name: name.into(),
        cursor: 0,
        storage: KeyStorage::File,
    };
    let record = SealedDeviceRecord::seal(
        &state.account_key,
        &state.account,
        state.device.public(),
        name,
        &mut OsRng,
    )
    .map_err(|e| e.to_string())?;
    remote.put_device_record(&state, &record.to_bytes())?;
    state.cursor = remote.latest_cursor(&state)?;
    Ok(state)
}
