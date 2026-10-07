//! The whole protocol end to end, the way the server and clients will run
//! it, plus a relay in the middle trying to tamper with every step.

use pastazzo_core::account::{AccountKey, WrappedAccountKey};
use pastazzo_core::device::{DeviceKeys, SealedDeviceRecord};
use pastazzo_core::invite::{Invite, InviteKey, InviteVerifier};
use pastazzo_core::item::{Content, ItemHeader, SealedItem};
use pastazzo_core::opaque::{self, ClientLoginResult};
use pastazzo_core::registration::RegistrationRecord;
use pastazzo_core::request::{RequestSignature, Scope};
use pastazzo_core::server::ServerKeys;
use pastazzo_core::{Error, Id, random_id, session};
use rand::rngs::OsRng;
use zeroize::Zeroizing;

const PASSWORD: &str = "correct horse battery staple";
const NOW: u64 = 1_791_379_434_274;

/// What the server keeps for an account.
struct Stored {
    username: String,
    account: Id,
    password_file: Vec<u8>,
    wrapped: WrappedAccountKey,
}

#[derive(Clone, Copy, PartialEq)]
enum Relay {
    Honest,
    /// Answers the registration request with its own OPRF key.
    SwapRegistrationResponse,
    /// Seals a registration record of its own.
    SwapRegistrationRecord,
    /// Registers its own device keys at login.
    SwapDeviceKeys,
    /// Hands the client a different wrapped account key.
    SwapWrappedKey,
}

/// First device: register with an invite, the way `pastazzo login` will.
/// The server only has the invite's verifier.
fn register(
    server: &ServerKeys,
    invite: &Invite,
    verifier: &InviteVerifier,
    relay: Relay,
) -> Result<Stored, Error> {
    let identity = server.identity();
    identity.verify(&invite.fingerprint)?;

    // Client: start.
    let (request, state) = opaque::client_registration_start(&mut OsRng, "seb", PASSWORD)?;
    let start_proof = invite.key.start_proof("seb", &request);

    // Server: the invite proof must verify before it answers.
    verifier.verify_start_proof("seb", &request, &start_proof)?;
    let mut response = opaque::server_registration_start(server, &request, "seb")?;
    if relay == Relay::SwapRegistrationResponse {
        let relay_keys = ServerKeys::generate(&mut OsRng);
        response.response =
            opaque::server_registration_start(&relay_keys, &request, "seb")?.response;
    }

    // Client: check the server signed the response, finish, create the
    // account key, seal the record.
    let result = state.finish(&mut OsRng, PASSWORD, &response, &identity)?;
    let account_key = AccountKey::generate(&mut OsRng);
    let account = random_id(&mut OsRng);
    let record = RegistrationRecord {
        username: "seb".into(),
        account,
        upload: result.upload,
        wrapped: WrappedAccountKey::wrap(&account_key, &result.export_key, &account, &mut OsRng),
    };
    let mut sealed = record.seal(&identity, &mut OsRng);
    let finish_proof = invite.key.finish_proof("seb", &sealed);

    if relay == Relay::SwapRegistrationRecord {
        // The relay knows the server's public transport key, so it can seal a
        // record of its own: the invite proof is what stops it.
        let (request, state) =
            opaque::client_registration_start(&mut OsRng, "seb", "relay's password")?;
        let response = opaque::server_registration_start(server, &request, "seb")?;
        let ours = state.finish(&mut OsRng, "relay's password", &response, &identity)?;
        sealed = RegistrationRecord {
            username: "seb".into(),
            account,
            upload: ours.upload,
            wrapped: WrappedAccountKey::wrap(
                &AccountKey::generate(&mut OsRng),
                &ours.export_key,
                &account,
                &mut OsRng,
            ),
        }
        .seal(&identity, &mut OsRng);
    }

    // Server: check the proof, open the record, store the password file.
    verifier.verify_finish_proof("seb", &sealed, &finish_proof)?;
    let record = RegistrationRecord::open(server, &sealed)?;
    assert_eq!(record.username, "seb");
    Ok(Stored {
        username: record.username,
        account: record.account,
        password_file: opaque::server_registration_finish(&record.upload)?,
        wrapped: record.wrapped,
    })
}

/// Any device: log in, register its keys, get the account key.
fn login(
    server: &ServerKeys,
    stored: &Stored,
    device: &DeviceKeys,
    password: &str,
    relay: Relay,
) -> Result<AccountKey, Error> {
    let identity = server.identity();

    // Client: start. Server: answer.
    let (request, client) = opaque::client_login_start(&mut OsRng, password)?;
    let (response, server_state) = opaque::server_login_start(
        &mut OsRng,
        server,
        Some(&stored.password_file),
        &request,
        &stored.username,
    )?;

    // Client: finish, bind this device's keys to the session.
    let ClientLoginResult {
        finalization,
        session_key,
        export_key,
    } = client.finish(&mut OsRng, password, &response, &identity)?;
    let mut device_public = device.public();
    let binding = session::device_binding_tag(&session_key, "seb", &device_public);
    if relay == Relay::SwapDeviceKeys {
        device_public = DeviceKeys::generate(&mut OsRng).public();
    }

    // Server: check the login and the device binding, answer with the
    // wrapped account key.
    let server_session = server_state.finish(&finalization)?;
    session::verify_device_binding(&server_session, "seb", &device_public, &binding)?;
    let tag = session::login_response_tag(&server_session, "seb", &stored.account, &stored.wrapped);
    let mut wrapped = stored.wrapped.clone();
    if relay == Relay::SwapWrappedKey {
        wrapped = WrappedAccountKey::wrap(
            &AccountKey::generate(&mut OsRng),
            &[0; 64],
            &stored.account,
            &mut OsRng,
        );
    }

    // Client: check the answer and unwrap the account key.
    session::verify_login_response(&session_key, "seb", &stored.account, &wrapped, &tag)?;
    wrapped.unwrap(&export_key, &stored.account)
}

/// An invite as the admin CLI makes it: the link for the user, the verifier
/// for the server, and the secret goes nowhere else.
fn invite_for(server: &ServerKeys) -> (Invite, InviteVerifier) {
    let invite = Invite {
        server_url: "https://clip.example.org".into(),
        fingerprint: server.identity().fingerprint(),
        key: InviteKey::generate(&mut OsRng),
    };
    let verifier = invite.key.verifier();
    let link: Zeroizing<String> = invite.to_link();
    (Invite::parse(&link).unwrap(), verifier)
}

fn registered(server: &ServerKeys) -> Stored {
    let (invite, verifier) = invite_for(server);
    register(server, &invite, &verifier, Relay::Honest).unwrap()
}

#[test]
fn two_devices_share_the_clipboard() {
    let server = ServerKeys::generate(&mut OsRng);
    let stored = registered(&server);

    let laptop = DeviceKeys::generate(&mut OsRng);
    let phone = DeviceKeys::generate(&mut OsRng);
    let laptop_key = login(&server, &stored, &laptop, PASSWORD, Relay::Honest).unwrap();
    let phone_key = login(&server, &stored, &phone, PASSWORD, Relay::Honest).unwrap();
    assert_eq!(laptop_key.expose_secret(), phone_key.expose_secret());
    let account = stored.account;

    // The laptop publishes its record; the phone checks it.
    let record =
        SealedDeviceRecord::seal(&laptop_key, &account, laptop.public(), "laptop", &mut OsRng)
            .unwrap();
    let record = SealedDeviceRecord::from_bytes(&record.to_bytes()).unwrap();
    assert_eq!(record.open(&phone_key, &account).unwrap(), "laptop");

    // The laptop copies; the server only ever handles bytes it can't read.
    let content = Content::Text("the wifi password is hunter2".into());
    let header = ItemHeader {
        id: random_id(&mut OsRng),
        device: laptop.id(),
        epoch: laptop_key.epoch(),
        created_at: NOW,
    };
    let upload = SealedItem::seal(&laptop_key, &account, header, &content, &mut OsRng)
        .unwrap()
        .to_bytes();
    assert!(!upload.windows(7).any(|w| w == b"hunter2"));

    let scope = Scope {
        server_fingerprint: server.identity().fingerprint(),
        account,
    };
    let signature = RequestSignature::sign(
        &laptop,
        &scope,
        "POST",
        "/v1/items",
        &upload,
        NOW,
        &mut OsRng,
    );
    signature
        .verify(&laptop.public(), &scope, "POST", "/v1/items", &upload, NOW)
        .unwrap();
    assert_eq!(SealedItem::from_bytes(&upload).unwrap().header, header);

    // The phone pastes.
    assert_eq!(
        SealedItem::from_bytes(&upload)
            .unwrap()
            .open(&phone_key, &account)
            .unwrap(),
        content
    );
}

#[test]
fn wrong_password_gets_nothing() {
    let server = ServerKeys::generate(&mut OsRng);
    let stored = registered(&server);
    let device = DeviceKeys::generate(&mut OsRng);
    assert_eq!(
        login(&server, &stored, &device, "guess", Relay::Honest).err(),
        Some(Error::Opaque)
    );
}

#[test]
fn relay_cannot_answer_the_registration_with_its_own_oprf_key() {
    let server = ServerKeys::generate(&mut OsRng);
    let (invite, verifier) = invite_for(&server);
    assert_eq!(
        register(&server, &invite, &verifier, Relay::SwapRegistrationResponse).err(),
        Some(Error::BadSignature)
    );
}

#[test]
fn relay_cannot_swap_the_registration_record() {
    let server = ServerKeys::generate(&mut OsRng);
    let (invite, verifier) = invite_for(&server);
    assert_eq!(
        register(&server, &invite, &verifier, Relay::SwapRegistrationRecord).err(),
        Some(Error::BadSignature)
    );
}

#[test]
fn relay_cannot_register_its_own_device_keys() {
    let server = ServerKeys::generate(&mut OsRng);
    let stored = registered(&server);
    let device = DeviceKeys::generate(&mut OsRng);
    assert_eq!(
        login(&server, &stored, &device, PASSWORD, Relay::SwapDeviceKeys).err(),
        Some(Error::BadMac)
    );
}

#[test]
fn relay_cannot_swap_the_account_key() {
    let server = ServerKeys::generate(&mut OsRng);
    let stored = registered(&server);
    let device = DeviceKeys::generate(&mut OsRng);
    assert_eq!(
        login(&server, &stored, &device, PASSWORD, Relay::SwapWrappedKey).err(),
        Some(Error::BadMac)
    );
}

#[test]
fn invite_for_another_server_is_refused() {
    let server = ServerKeys::generate(&mut OsRng);
    let other = ServerKeys::generate(&mut OsRng);
    let (invite, verifier) = invite_for(&other);
    assert_eq!(
        register(&server, &invite, &verifier, Relay::Honest).err(),
        Some(Error::UnknownServer)
    );
}

#[test]
fn stolen_wrapped_key_is_useless_without_the_password() {
    let server = ServerKeys::generate(&mut OsRng);
    let stored = registered(&server);
    // Without the password there is no export key to unwrap with.
    for guess in [[0u8; 64], [1u8; 64]] {
        assert_eq!(
            stored.wrapped.unwrap(&guess, &stored.account).err(),
            Some(Error::Decrypt)
        );
    }
}
