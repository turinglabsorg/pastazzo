//! Creating an account, logging devices in and approving them, following
//! `docs/PROTOCOL.md`.
//!
//! An account is a small network of devices sharing one account key. The
//! first device creates it, with an invite. Every other device logs in with
//! the password and then has to be approved by a device already in the
//! account, which hands it the account secret: the password alone doesn't
//! open the account key.

use std::time::{Duration, Instant};

use pastazzo_core::account::{AccountKey, AccountSecret, WrappedAccountKey};
use pastazzo_core::api::{
    B64, LoginFinish, LoginFinished, LoginStart, LoginStarted, RegisterFinish, RegisterStart,
    RegisterStarted,
};
use pastazzo_core::approval::{approval_code, open_grant, seal_grant};
use pastazzo_core::device::{DeviceKeys, DevicePublic, SealedDeviceRecord};
use pastazzo_core::invite::Invite;
use pastazzo_core::opaque::{self, SignedRegistrationResponse};
use pastazzo_core::registration::RegistrationRecord;
use pastazzo_core::request::Scope;
use pastazzo_core::server::ServerIdentity;
use pastazzo_core::{Id, random_id, session};
use rand::rngs::OsRng;
use zeroize::Zeroizing;

use crate::Result;
use crate::remote::Remote;
use crate::state::{KeyStorage, State};

/// How long a new device waits for someone to approve it.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const APPROVAL_POLL: Duration = Duration::from_secs(2);

/// Fetches the server identity and checks it against the pinned fingerprint.
fn pinned_identity(remote: &Remote, fingerprint: &[u8; 32]) -> Result<ServerIdentity> {
    let info = remote.server_info()?;
    if info.version != pastazzo_core::PROTOCOL_VERSION {
        return Err(format!(
            "the server speaks protocol version {}, this client {}",
            info.version,
            pastazzo_core::PROTOCOL_VERSION
        ));
    }
    let identity = ServerIdentity::from_bytes(&info.identity.0)
        .map_err(|_| "the server sent an invalid identity")?;
    identity.verify(fingerprint).map_err(
        |_| "the server's identity doesn't match the fingerprint: refusing to talk to it",
    )?;
    Ok(identity)
}

/// Creates the account with an invite link, as its first device: a new
/// network of devices, with a new account key and account secret.
pub fn join(link: &str, username: &str, password: &str, device_name: &str) -> Result<State> {
    let invite = Invite::parse(link).map_err(|_| "that isn't a valid pastazzo invite link")?;
    let remote = Remote::new(&invite.server_url);
    let identity = pinned_identity(&remote, &invite.fingerprint)?;

    let (request, state) = opaque::client_registration_start(&mut OsRng, username, password)
        .map_err(|e| e.to_string())?;
    let started: RegisterStarted = remote.post_json(
        "/v1/register/start",
        &RegisterStart {
            username: username.to_owned(),
            proof: Some(B64(invite.key.start_proof(username, &request).to_vec())),
            request: B64(request),
            invite_id: Some(B64(invite.key.id.to_vec())),
        },
    )?;
    let response = SignedRegistrationResponse {
        response: started.response.0,
        signature: started
            .signature
            .array()
            .ok_or("invalid server signature")?,
    };
    // Checks the server's signature before anything derives from the password.
    let result = state
        .finish(&mut OsRng, password, &response, &identity)
        .map_err(|e| format!("registration refused: {e}"))?;

    let account_key = AccountKey::generate(&mut OsRng);
    let secret = AccountSecret::generate(&mut OsRng);
    let account = random_id(&mut OsRng);
    let sealed = RegistrationRecord {
        username: username.to_owned(),
        account,
        wrapped: WrappedAccountKey::wrap(
            &account_key,
            &result.export_key,
            &secret,
            &account,
            &mut OsRng,
        ),
        upload: result.upload,
    }
    .seal(&identity, &mut OsRng);
    let proof = invite.key.finish_proof(username, &sealed);
    remote.post_json_empty(
        "/v1/register/finish",
        &RegisterFinish {
            registration_id: started.registration_id,
            sealed: B64(sealed),
            proof: Some(B64(proof.to_vec())),
        },
    )?;

    // Logging in right away also proves the registration wasn't tampered
    // with. As the account's first device, this one approves itself.
    log_in(&invite.server_url, &invite.fingerprint, username, password, device_name, Some(secret), &mut |_| {})
        .map_err(|e| {
            format!("the account was created but logging in failed, so the registration may have been tampered with: {e}")
        })
}

/// Logs another device in: once the password checks out, it shows
/// `on_code` its approval code and waits for a device already in the
/// account to approve it.
pub fn login(
    server_url: &str,
    fingerprint: &[u8; 32],
    username: &str,
    password: &str,
    device_name: &str,
    on_code: &mut dyn FnMut(&str),
) -> Result<State> {
    log_in(
        server_url,
        fingerprint,
        username,
        password,
        device_name,
        None,
        on_code,
    )
}

fn log_in(
    server_url: &str,
    fingerprint: &[u8; 32],
    username: &str,
    password: &str,
    device_name: &str,
    known_secret: Option<AccountSecret>,
    on_code: &mut dyn FnMut(&str),
) -> Result<State> {
    let remote = Remote::new(server_url);
    let identity = pinned_identity(&remote, fingerprint)?;
    let device = DeviceKeys::generate(&mut OsRng);

    let (request, client) =
        opaque::client_login_start(&mut OsRng, password).map_err(|e| e.to_string())?;
    let started: LoginStarted = remote.post_json(
        "/v1/login/start",
        &LoginStart {
            username: username.to_owned(),
            request: B64(request),
        },
    )?;
    let result = client
        .finish(&mut OsRng, password, &started.response.0, &identity)
        .map_err(|_| "wrong username or password")?;
    let binding = session::device_binding_tag(&result.session_key, username, &device.public());
    let finished: LoginFinished = remote.post_json(
        "/v1/login/finish",
        &LoginFinish {
            login_id: started.login_id,
            finalization: B64(result.finalization),
            device: B64(device.public().to_bytes().to_vec()),
            binding: B64(binding.to_vec()),
        },
    )?;

    let account = finished.account.array().ok_or("invalid server response")?;
    let wrapped = WrappedAccountKey::from_bytes(&finished.wrapped.0)
        .map_err(|_| "invalid server response")?;
    session::verify_login_response(
        &result.session_key,
        username,
        &account,
        &wrapped,
        &finished.tag.0,
    )
    .map_err(|_| "the login response was tampered with")?;
    let export_key = Zeroizing::new(result.export_key.to_vec());

    let scope = Scope {
        server_fingerprint: identity.fingerprint(),
        account,
    };
    let secret = match known_secret {
        Some(secret) => secret,
        None => wait_for_approval(&remote, &device, &scope, on_code)?,
    };
    let account_key = wrapped.unwrap(&export_key, &secret, &account).map_err(
        |_| "couldn't open the account key: the approval didn't come from a device of this account",
    )?;

    let mut state = State {
        server_url: server_url.trim_end_matches('/').to_owned(),
        identity,
        username: username.to_owned(),
        account,
        account_key,
        account_secret: Some(secret),
        device,
        device_name: device_name.to_owned(),
        cursor: 0,
        // Where the keys go is decided when the state is first saved.
        storage: KeyStorage::File,
    };
    let record = SealedDeviceRecord::seal(
        &state.account_key,
        &state.account,
        state.device.public(),
        device_name,
        &mut OsRng,
    )
    .map_err(|_| "device name too long (at most 128 bytes)")?;
    remote.put_device_record(&state, &record.to_bytes())?;
    // A new device starts from now, not from the account's whole history.
    state.cursor = remote.latest_cursor(&state)?;
    Ok(state)
}

fn wait_for_approval(
    remote: &Remote,
    device: &DeviceKeys,
    scope: &Scope,
    on_code: &mut dyn FnMut(&str),
) -> Result<AccountSecret> {
    on_code(&approval_code(&device.public()));
    let started = Instant::now();
    loop {
        if let Some(grant) = remote.grant(device, scope)? {
            return open_grant(device, &scope.account, &grant)
                .map_err(|_| "the approval can't be opened by this device".to_owned());
        }
        if started.elapsed() > APPROVAL_TIMEOUT {
            return Err("nobody approved this device in time: log in again to retry".to_owned());
        }
        std::thread::sleep(APPROVAL_POLL);
    }
}

/// A device of the account, as its record decrypts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: Id,
    pub name: String,
    /// SHA-256 of its public keys, to compare with what the device itself shows.
    pub fingerprint: [u8; 32],
    pub this: bool,
}

/// A device that logged in and waits for approval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDevice {
    pub public: DevicePublic,
    /// What the waiting device shows: approve only if it's the same.
    pub code: String,
}

/// The account's devices and the ones waiting for approval. Records that
/// don't decrypt weren't made by a holder of the account key, and are left
/// out.
pub fn devices_and_pending(state: &State) -> Result<(Vec<DeviceInfo>, Vec<PendingDevice>)> {
    let list = Remote::new(&state.server_url).devices(state)?;
    let mut devices = Vec::new();
    for record in list.records {
        let Ok(record) = SealedDeviceRecord::from_bytes(&record.0) else {
            continue;
        };
        if let Ok(name) = record.open(&state.account_key, &state.account) {
            devices.push(DeviceInfo {
                id: record.device.id,
                name,
                fingerprint: record.device.fingerprint(),
                this: record.device.id == state.device.id(),
            });
        }
    }
    let pending = list
        .pending
        .iter()
        .filter_map(|public| DevicePublic::from_bytes(&public.0).ok())
        .map(|public| PendingDevice {
            code: approval_code(&public),
            public,
        })
        .collect();
    Ok((devices, pending))
}

pub fn devices(state: &State) -> Result<Vec<DeviceInfo>> {
    Ok(devices_and_pending(state)?.0)
}

/// Names of the account's devices, as their records decrypt.
pub fn device_names(state: &State) -> Result<Vec<(String, bool)>> {
    Ok(devices(state)?
        .into_iter()
        .map(|d| (d.name, d.this))
        .collect())
}

/// Approves a waiting device: hands it the account secret, sealed to its
/// keys. Whoever approves must have checked the code first.
pub fn approve(state: &State, device: &Id) -> Result<PendingDevice> {
    let secret = state.account_secret.as_ref().ok_or(
        "this device's account was made before approvals existed: create the account again to approve devices",
    )?;
    let (_, pending) = devices_and_pending(state)?;
    let waiting = pending
        .into_iter()
        .find(|p| p.public.id == *device)
        .ok_or("no device with that id is waiting for approval")?;
    let grant = seal_grant(secret, &state.account, &waiting.public, &mut OsRng);
    Remote::new(&state.server_url).put_grant(state, device, &grant)?;
    Ok(waiting)
}
