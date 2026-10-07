//! Registration and login with OPAQUE (RFC 9807).
//!
//! The server never learns the password, nor anything that would let a relay
//! watching the traffic test password guesses offline. Every login gives the
//! client an `export_key` only it can compute, used to unwrap the account
//! key, and a session key shared with the server, used to bind the device
//! keys to this login.
//!
//! OPAQUE assumes registration happens over an authenticated channel, which
//! a relay isn't: it could answer the registration request with an OPRF
//! evaluation under a key of its own and then guess the password offline. So
//! the server signs its registration response, and the client checks the
//! signature against the pinned server identity before finishing. Login
//! needs no extra step: OPAQUE authenticates the server there.
//!
//! Cipher suite: Ristretto255 OPRF, 3DH over Ristretto255 with SHA-512, and
//! Argon2id (64 MiB, 3 passes, 4 lanes: RFC 9106's second recommended
//! option) as the key stretching function. These parameters are part of the
//! protocol: changing them changes every password file.

use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialFinalization, CredentialRequest,
    CredentialResponse, Identifiers, RegistrationRequest, RegistrationResponse, RegistrationUpload,
    ServerLogin, ServerLoginParameters, ServerRegistration,
};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::server::{ServerIdentity, ServerKeys};
use crate::{Error, Rng, transcript};

pub struct Suite;

impl opaque_ke::CipherSuite for Suite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = opaque_ke::TripleDh<opaque_ke::Ristretto255, sha2::Sha512>;
    type Ksf = argon2::Argon2<'static>;
}

/// OPAQUE context string: both sides must agree on it for a login to work.
const CONTEXT: &[u8] = b"pastazzo/v1";

const ARGON2_MEMORY_KIB: u32 = 64 * 1024;
const ARGON2_PASSES: u32 = 3;
const ARGON2_LANES: u32 = 4;

fn ksf() -> argon2::Argon2<'static> {
    let params = argon2::Params::new(ARGON2_MEMORY_KIB, ARGON2_PASSES, ARGON2_LANES, None)
        .expect("valid Argon2id parameters");
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
}

/// Passwords are UTF-8 in Unicode NFC, so the same password typed on
/// different platforms gives the same bytes.
fn normalize(password: &str) -> Zeroizing<String> {
    // NFC expands UTF-8 by at most 3×: reserving that much means the buffer
    // never reallocates and leaves no unzeroed copy behind.
    let mut normalized = Zeroizing::new(String::with_capacity(password.len() * 3));
    normalized.extend(password.nfc());
    normalized
}

/// Usernames are 1 to 64 characters of `a-z 0-9 . _ -`.
pub fn validate_username(username: &str) -> Result<(), Error> {
    let valid = (1..=64).contains(&username.len())
        && username
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
    if valid {
        Ok(())
    } else {
        Err(Error::Malformed("username"))
    }
}

fn check_server(server_s_pk: &[u8], pinned: &ServerIdentity) -> Result<(), Error> {
    if server_s_pk == pinned.opaque {
        Ok(())
    } else {
        Err(Error::UnknownServer)
    }
}

fn registration_response_message(username: &str, request: &[u8], response: &[u8]) -> Vec<u8> {
    transcript(
        "pastazzo/v1/registration-response",
        &[username.as_bytes(), request, response],
    )
}

/// The server's registration response, signed with its identity key.
pub struct SignedRegistrationResponse {
    pub response: Vec<u8>,
    pub signature: [u8; 64],
}

// Client side.

pub struct ClientRegistrationState {
    state: ClientRegistration<Suite>,
    username: String,
    request: Vec<u8>,
}

pub struct ClientRegistrationResult {
    /// The OPAQUE registration record, to seal to the server. As sensitive
    /// as a password hash.
    pub upload: Zeroizing<Vec<u8>>,
    pub export_key: Zeroizing<Vec<u8>>,
}

/// First registration message. Send the returned bytes to the server.
pub fn client_registration_start(
    rng: &mut impl Rng,
    username: &str,
    password: &str,
) -> Result<(Vec<u8>, ClientRegistrationState), Error> {
    validate_username(username)?;
    let start = ClientRegistration::<Suite>::start(rng, normalize(password).as_bytes())?;
    let request = start.message.serialize().to_vec();
    let state = ClientRegistrationState {
        state: start.state,
        username: username.to_owned(),
        request: request.clone(),
    };
    Ok((request, state))
}

impl ClientRegistrationState {
    /// Fails with [`Error::BadSignature`] if the response wasn't signed by the
    /// pinned server, before anything derived from the password is computed.
    pub fn finish(
        self,
        rng: &mut impl Rng,
        password: &str,
        response: &SignedRegistrationResponse,
        server: &ServerIdentity,
    ) -> Result<ClientRegistrationResult, Error> {
        server.verify_signature(
            &registration_response_message(&self.username, &self.request, &response.response),
            &response.signature,
        )?;
        let ksf = ksf();
        let finish = self.state.finish(
            rng,
            normalize(password).as_bytes(),
            RegistrationResponse::deserialize(&response.response)?,
            ClientRegistrationFinishParameters::new(Identifiers::default(), Some(&ksf)),
        )?;
        check_server(&finish.server_s_pk.serialize(), server)?;
        Ok(ClientRegistrationResult {
            upload: Zeroizing::new(finish.message.serialize().to_vec()),
            export_key: Zeroizing::new(finish.export_key.to_vec()),
        })
    }
}

pub struct ClientLoginState(ClientLogin<Suite>);

pub struct ClientLoginResult {
    /// Last login message, for the server.
    pub finalization: Vec<u8>,
    pub session_key: Zeroizing<Vec<u8>>,
    pub export_key: Zeroizing<Vec<u8>>,
}

/// First login message. Send the returned bytes to the server.
pub fn client_login_start(
    rng: &mut impl Rng,
    password: &str,
) -> Result<(Vec<u8>, ClientLoginState), Error> {
    let start = ClientLogin::<Suite>::start(rng, normalize(password).as_bytes())?;
    Ok((
        start.message.serialize().to_vec(),
        ClientLoginState(start.state),
    ))
}

impl ClientLoginState {
    /// Fails with [`Error::Opaque`] on a wrong password or tampered messages,
    /// and with [`Error::UnknownServer`] if the server isn't the pinned one.
    pub fn finish(
        self,
        rng: &mut impl Rng,
        password: &str,
        response: &[u8],
        server: &ServerIdentity,
    ) -> Result<ClientLoginResult, Error> {
        let ksf = ksf();
        let finish = self.0.finish(
            rng,
            normalize(password).as_bytes(),
            CredentialResponse::deserialize(response)?,
            ClientLoginFinishParameters::new(Some(CONTEXT), Identifiers::default(), Some(&ksf)),
        )?;
        check_server(&finish.server_s_pk.serialize(), server)?;
        Ok(ClientLoginResult {
            finalization: finish.message.serialize().to_vec(),
            session_key: Zeroizing::new(finish.session_key.to_vec()),
            export_key: Zeroizing::new(finish.export_key.to_vec()),
        })
    }
}

// Server side.

/// Answers the first registration message, signed.
pub fn server_registration_start(
    keys: &ServerKeys,
    request: &[u8],
    username: &str,
) -> Result<SignedRegistrationResponse, Error> {
    validate_username(username)?;
    let start = ServerRegistration::<Suite>::start(
        keys.opaque(),
        RegistrationRequest::deserialize(request)?,
        username.as_bytes(),
    )?;
    let response = start.message.serialize().to_vec();
    let signature = keys.sign(&registration_response_message(username, request, &response));
    Ok(SignedRegistrationResponse {
        response,
        signature,
    })
}

/// Turns the client's registration record into the password file to store.
pub fn server_registration_finish(upload: &[u8]) -> Result<Vec<u8>, Error> {
    Ok(
        ServerRegistration::<Suite>::finish(RegistrationUpload::deserialize(upload)?)
            .serialize()
            .to_vec(),
    )
}

/// Server state between the two login messages. It holds secrets: keep it in
/// memory, for a short time, and only once.
pub struct ServerLoginState(ServerLogin<Suite>);

/// Answers the first login message. Pass `None` as the password file for an
/// unknown username: the response then looks like a real one, so usernames
/// can't be enumerated.
pub fn server_login_start(
    rng: &mut impl Rng,
    keys: &ServerKeys,
    password_file: Option<&[u8]>,
    request: &[u8],
    username: &str,
) -> Result<(Vec<u8>, ServerLoginState), Error> {
    validate_username(username)?;
    let password_file = password_file
        .map(ServerRegistration::<Suite>::deserialize)
        .transpose()?;
    let start = ServerLogin::start(
        rng,
        keys.opaque(),
        password_file,
        CredentialRequest::deserialize(request)?,
        username.as_bytes(),
        ServerLoginParameters {
            context: Some(CONTEXT),
            identifiers: Identifiers::default(),
        },
    )?;
    Ok((
        start.message.serialize().to_vec(),
        ServerLoginState(start.state),
    ))
}

impl ServerLoginState {
    /// Checks the client's last message. Success means the client knows the
    /// password; the returned session key matches the client's.
    pub fn finish(self, finalization: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
        let finish = self.0.finish(
            CredentialFinalization::deserialize(finalization)?,
            ServerLoginParameters {
                context: Some(CONTEXT),
                identifiers: Identifiers::default(),
            },
        )?;
        Ok(Zeroizing::new(finish.session_key.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn register(
        keys: &ServerKeys,
        username: &str,
        password: &str,
    ) -> (Vec<u8>, Zeroizing<Vec<u8>>) {
        let (request, state) = client_registration_start(&mut OsRng, username, password).unwrap();
        let response = server_registration_start(keys, &request, username).unwrap();
        let result = state
            .finish(&mut OsRng, password, &response, &keys.identity())
            .unwrap();
        (
            server_registration_finish(&result.upload).unwrap(),
            result.export_key,
        )
    }

    fn login(
        keys: &ServerKeys,
        file: Option<&[u8]>,
        username: &str,
        password: &str,
    ) -> Result<(ClientLoginResult, Zeroizing<Vec<u8>>), Error> {
        let (request, client) = client_login_start(&mut OsRng, password)?;
        let (response, server) = server_login_start(&mut OsRng, keys, file, &request, username)?;
        let client = client.finish(&mut OsRng, password, &response, &keys.identity())?;
        let server = server.finish(&client.finalization)?;
        Ok((client, server))
    }

    #[test]
    fn register_and_login() {
        let keys = ServerKeys::generate(&mut OsRng);
        let (file, registration_export_key) =
            register(&keys, "seb", "correct horse battery staple");
        let (client, server_session) =
            login(&keys, Some(&file), "seb", "correct horse battery staple").unwrap();
        assert_eq!(client.session_key, server_session);
        assert_eq!(client.export_key, registration_export_key);
    }

    #[test]
    fn password_is_unicode_normalized() {
        let keys = ServerKeys::generate(&mut OsRng);
        let (file, _) = register(&keys, "seb", "caf\u{e9}");
        assert!(login(&keys, Some(&file), "seb", "cafe\u{301}").is_ok());
    }

    #[test]
    fn wrong_password_fails() {
        let keys = ServerKeys::generate(&mut OsRng);
        let (file, _) = register(&keys, "seb", "right");
        assert_eq!(
            login(&keys, Some(&file), "seb", "wrong").err(),
            Some(Error::Opaque)
        );
    }

    #[test]
    fn password_file_is_bound_to_the_username() {
        let keys = ServerKeys::generate(&mut OsRng);
        let (file, _) = register(&keys, "seb", "pw");
        assert_eq!(
            login(&keys, Some(&file), "other", "pw").err(),
            Some(Error::Opaque)
        );
    }

    #[test]
    fn unknown_user_gets_a_plausible_response() {
        let keys = ServerKeys::generate(&mut OsRng);
        let (request, client) = client_login_start(&mut OsRng, "pw").unwrap();
        let (response, _) =
            server_login_start(&mut OsRng, &keys, None, &request, "nobody").unwrap();
        let (file, _) = register(&keys, "seb", "pw");
        let (real_request, _) = client_login_start(&mut OsRng, "pw").unwrap();
        let (real_response, _) =
            server_login_start(&mut OsRng, &keys, Some(&file), &real_request, "seb").unwrap();
        assert_eq!(response.len(), real_response.len());
        assert_eq!(
            client
                .finish(&mut OsRng, "pw", &response, &keys.identity())
                .err(),
            Some(Error::Opaque)
        );
    }

    #[test]
    fn relay_cannot_answer_the_registration() {
        // The relay evaluates the OPRF with its own key and forwards the real
        // server's public key: without the server's signature the client stops.
        let server = ServerKeys::generate(&mut OsRng);
        let relay = ServerKeys::generate(&mut OsRng);
        let (request, state) = client_registration_start(&mut OsRng, "seb", "pw").unwrap();
        let mut forged = server_registration_start(&relay, &request, "seb").unwrap();
        let real = server_registration_start(&server, &request, "seb").unwrap();
        assert_eq!(
            state
                .finish(&mut OsRng, "pw", &forged, &server.identity())
                .err(),
            Some(Error::BadSignature)
        );

        // Reusing the real signature on the forged response doesn't help.
        let (request, state) = client_registration_start(&mut OsRng, "seb", "pw").unwrap();
        forged = server_registration_start(&relay, &request, "seb").unwrap();
        forged.signature = server_registration_start(&server, &request, "seb")
            .unwrap()
            .signature;
        assert_eq!(
            state
                .finish(&mut OsRng, "pw", &forged, &server.identity())
                .err(),
            Some(Error::BadSignature)
        );

        // Nor does replaying a real response made for another request.
        let (_, state) = client_registration_start(&mut OsRng, "seb", "pw").unwrap();
        assert_eq!(
            state
                .finish(&mut OsRng, "pw", &real, &server.identity())
                .err(),
            Some(Error::BadSignature)
        );
    }

    #[test]
    fn impostor_server_is_detected() {
        let keys = ServerKeys::generate(&mut OsRng);
        let pinned = ServerKeys::generate(&mut OsRng).identity();

        let (request, state) = client_registration_start(&mut OsRng, "seb", "pw").unwrap();
        let response = server_registration_start(&keys, &request, "seb").unwrap();
        assert_eq!(
            state.finish(&mut OsRng, "pw", &response, &pinned).err(),
            Some(Error::BadSignature)
        );

        let (file, _) = register(&keys, "seb", "pw");
        let (request, client) = client_login_start(&mut OsRng, "pw").unwrap();
        let (response, _) =
            server_login_start(&mut OsRng, &keys, Some(&file), &request, "seb").unwrap();
        assert_eq!(
            client.finish(&mut OsRng, "pw", &response, &pinned).err(),
            Some(Error::UnknownServer)
        );
    }

    #[test]
    fn usernames() {
        for ok in ["seb", "a", "first.last_2-x", &"a".repeat(64)] {
            assert!(validate_username(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Seb", "a b", "a/b", "é", &"a".repeat(65)] {
            assert!(validate_username(bad).is_err(), "{bad}");
        }
    }
}
