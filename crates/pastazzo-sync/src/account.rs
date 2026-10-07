//! Joining with an invite and logging in, following `docs/PROTOCOL.md`.

use pastazzo_core::account::{AccountKey, WrappedAccountKey};
use pastazzo_core::api::{
    B64, LoginFinish, LoginFinished, LoginStart, LoginStarted, RegisterFinish, RegisterStart,
    RegisterStarted,
};
use pastazzo_core::device::{DeviceKeys, SealedDeviceRecord};
use pastazzo_core::invite::Invite;
use pastazzo_core::opaque::{self, SignedRegistrationResponse};
use pastazzo_core::registration::RegistrationRecord;
use pastazzo_core::server::ServerIdentity;
use pastazzo_core::{random_id, session};
use rand::rngs::OsRng;

use crate::Result;
use crate::remote::Remote;
use crate::state::State;

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

/// Creates the account with an invite link, then logs this device in.
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
    let account = random_id(&mut OsRng);
    let sealed = RegistrationRecord {
        username: username.to_owned(),
        account,
        wrapped: WrappedAccountKey::wrap(&account_key, &result.export_key, &account, &mut OsRng),
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

    // Logging in right away also proves the registration wasn't tampered with.
    login(&invite.server_url, &invite.fingerprint, username, password, device_name)
        .map_err(|e| format!("the account was created but logging in failed, so the registration may have been tampered with: {e}"))
}

/// Logs this device in: registers its keys and gets the account key.
pub fn login(
    server_url: &str,
    fingerprint: &[u8; 32],
    username: &str,
    password: &str,
    device_name: &str,
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
    let account_key = wrapped
        .unwrap(&result.export_key, &account)
        .map_err(|_| "couldn't open the account key")?;

    let mut state = State {
        server_url: server_url.trim_end_matches('/').to_owned(),
        identity,
        username: username.to_owned(),
        account,
        account_key,
        device,
        device_name: device_name.to_owned(),
        cursor: 0,
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

/// A device of the account, as its record decrypts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: pastazzo_core::Id,
    pub name: String,
    /// SHA-256 of its public keys, to compare with what the device itself shows.
    pub fingerprint: [u8; 32],
    pub this: bool,
}

/// The account's devices. Records that don't decrypt weren't made by a
/// holder of the account key, and are left out.
pub fn devices(state: &State) -> Result<Vec<DeviceInfo>> {
    let remote = Remote::new(&state.server_url);
    let mut devices = Vec::new();
    for record in remote.device_records(state)? {
        let Ok(record) = SealedDeviceRecord::from_bytes(&record) else {
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
    Ok(devices)
}

/// Names of the account's devices, as their records decrypt.
pub fn device_names(state: &State) -> Result<Vec<(String, bool)>> {
    Ok(devices(state)?
        .into_iter()
        .map(|d| (d.name, d.this))
        .collect())
}
