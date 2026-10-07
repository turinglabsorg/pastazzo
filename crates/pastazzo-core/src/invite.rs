//! Invites.
//!
//! An invite link carries the server URL, the server's identity fingerprint
//! and a one-time secret. The secret is never sent anywhere and the server
//! doesn't keep it: it keeps only an Ed25519 public key derived from it. The
//! client signs each registration message with the matching private key, so
//! a relay that sees the registration can neither reuse the invite nor swap
//! the messages for its own, and a stolen server database holds no usable
//! invites.
//!
//! Link format:
//!
//! ```text
//! pastazzo://join?v=1&server=<url>&fp=<fingerprint>&id=<invite id>&key=<secret>
//! ```
//!
//! `server`, `fp`, `id` and `key` are base64url without padding. The link is
//! printed by the server's admin CLI and must reach the user out of band.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use zeroize::Zeroizing;

use crate::{Error, Id, Rng, derive_key, random_id, transcript};

const LINK_PREFIX: &str = "pastazzo://join?";

/// The invite secret, as the admin CLI creates it and the client reads it
/// from the link.
pub struct InviteKey {
    pub id: Id,
    secret: Zeroizing<[u8; 32]>,
}

impl InviteKey {
    pub fn generate(rng: &mut impl Rng) -> Self {
        let mut secret = Zeroizing::new([0u8; 32]);
        rng.fill_bytes(secret.as_mut());
        Self {
            id: random_id(rng),
            secret,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_secret(id: Id, secret: [u8; 32]) -> Self {
        Self {
            id,
            secret: Zeroizing::new(secret),
        }
    }

    fn signing_key(&self) -> SigningKey {
        let seed = derive_key(
            self.secret.as_ref(),
            &transcript("pastazzo/v1/invite-key", &[&self.id]),
        );
        SigningKey::from_bytes(&seed)
    }

    /// What the server stores for this invite.
    pub fn verifier(&self) -> InviteVerifier {
        InviteVerifier {
            id: self.id,
            public: self.signing_key().verifying_key().to_bytes(),
        }
    }

    /// Proof for the first registration message.
    pub fn start_proof(&self, username: &str, request: &[u8]) -> [u8; 64] {
        self.signing_key()
            .sign(&start_message(&self.id, username, request))
            .to_bytes()
    }

    /// Proof for the sealed registration record.
    pub fn finish_proof(&self, username: &str, sealed_record: &[u8]) -> [u8; 64] {
        self.signing_key()
            .sign(&finish_message(&self.id, username, sealed_record))
            .to_bytes()
    }
}

/// The server's side of an invite: its id and the public key the client's
/// proofs verify against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InviteVerifier {
    pub id: Id,
    pub public: [u8; 32],
}

impl InviteVerifier {
    pub fn verify_start_proof(
        &self,
        username: &str,
        request: &[u8],
        proof: &[u8],
    ) -> Result<(), Error> {
        self.verify(&start_message(&self.id, username, request), proof)
    }

    pub fn verify_finish_proof(
        &self,
        username: &str,
        sealed_record: &[u8],
        proof: &[u8],
    ) -> Result<(), Error> {
        self.verify(&finish_message(&self.id, username, sealed_record), proof)
    }

    fn verify(&self, message: &[u8], proof: &[u8]) -> Result<(), Error> {
        let proof: [u8; 64] = proof.try_into().map_err(|_| Error::BadSignature)?;
        VerifyingKey::from_bytes(&self.public)
            .map_err(|_| Error::Malformed("invite verifier"))?
            .verify_strict(message, &Signature::from_bytes(&proof))
            .map_err(|_| Error::BadSignature)
    }
}

fn start_message(id: &Id, username: &str, request: &[u8]) -> Vec<u8> {
    transcript(
        "pastazzo/v1/invite/registration-start",
        &[id, username.as_bytes(), request],
    )
}

fn finish_message(id: &Id, username: &str, sealed_record: &[u8]) -> Vec<u8> {
    transcript(
        "pastazzo/v1/invite/registration-finish",
        &[id, username.as_bytes(), sealed_record],
    )
}

/// A parsed invite link.
pub struct Invite {
    pub server_url: String,
    pub fingerprint: [u8; 32],
    pub key: InviteKey,
}

impl Invite {
    pub fn to_link(&self) -> Zeroizing<String> {
        Zeroizing::new(format!(
            "{LINK_PREFIX}v=1&server={}&fp={}&id={}&key={}",
            URL_SAFE_NO_PAD.encode(self.server_url.as_bytes()),
            URL_SAFE_NO_PAD.encode(self.fingerprint),
            URL_SAFE_NO_PAD.encode(self.key.id),
            Zeroizing::new(URL_SAFE_NO_PAD.encode(self.key.secret.as_ref())).as_str(),
        ))
    }

    pub fn parse(link: &str) -> Result<Self, Error> {
        let malformed = Error::Malformed("invite link");
        let query = link
            .trim()
            .strip_prefix(LINK_PREFIX)
            .ok_or(malformed.clone())?;
        let (mut version, mut server, mut fingerprint, mut id, mut key) =
            (None, None, None, None, None);
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').ok_or(malformed.clone())?;
            let slot = match name {
                "v" => &mut version,
                "server" => &mut server,
                "fp" => &mut fingerprint,
                "id" => &mut id,
                "key" => &mut key,
                _ => return Err(malformed),
            };
            if slot.replace(value).is_some() {
                return Err(malformed);
            }
        }
        if version != Some("1") {
            return Err(malformed);
        }
        let decode = |value: Option<&str>| {
            URL_SAFE_NO_PAD
                .decode(value.ok_or(Error::Malformed("invite link"))?)
                .map(Zeroizing::new)
                .map_err(|_| Error::Malformed("invite link"))
        };
        let server_url =
            String::from_utf8(decode(server)?.to_vec()).map_err(|_| malformed.clone())?;
        if !(server_url.starts_with("https://") || server_url.starts_with("http://")) {
            return Err(malformed);
        }
        let fingerprint = decode(fingerprint)?
            .as_slice()
            .try_into()
            .map_err(|_| malformed.clone())?;
        let id = decode(id)?
            .as_slice()
            .try_into()
            .map_err(|_| malformed.clone())?;
        let encoded = decode(key)?;
        if encoded.len() != 32 {
            return Err(malformed);
        }
        let mut secret = Zeroizing::new([0u8; 32]);
        secret.copy_from_slice(&encoded);
        Ok(Self {
            server_url,
            fingerprint,
            key: InviteKey { id, secret },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn invite() -> Invite {
        Invite {
            server_url: "https://clip.example.org".into(),
            fingerprint: [5; 32],
            key: InviteKey::generate(&mut OsRng),
        }
    }

    #[test]
    fn link_roundtrip() {
        let invite = invite();
        let parsed = Invite::parse(&invite.to_link()).unwrap();
        assert_eq!(parsed.server_url, invite.server_url);
        assert_eq!(parsed.fingerprint, invite.fingerprint);
        assert_eq!(parsed.key.verifier(), invite.key.verifier());
    }

    #[test]
    fn bad_links_are_rejected() {
        let link = invite().to_link().to_string();
        for bad in [
            link.replace("v=1", "v=2"),
            link.replace("pastazzo://join?", "https://join?"),
            format!("{link}&v=1"),
            format!("{link}&extra=1"),
            link.split("&key=").next().unwrap().to_string(),
            link.replace("&fp=", "&fp=AAAA"),
            link.replace("&key=", "&key=AAAA"),
        ] {
            assert!(Invite::parse(&bad).is_err(), "{bad}");
        }
        let mut not_http = invite();
        not_http.server_url = "file:///etc/passwd".into();
        assert!(Invite::parse(&not_http.to_link()).is_err());
    }

    #[test]
    fn proofs_bind_everything() {
        let key = InviteKey::generate(&mut OsRng);
        let verifier = key.verifier();
        let proof = key.start_proof("seb", b"request");
        assert_eq!(
            verifier.verify_start_proof("seb", b"request", &proof),
            Ok(())
        );
        assert!(
            verifier
                .verify_start_proof("other", b"request", &proof)
                .is_err()
        );
        assert!(
            verifier
                .verify_start_proof("seb", b"other request", &proof)
                .is_err()
        );
        assert!(
            verifier
                .verify_start_proof("seb", b"request", &proof[..63])
                .is_err()
        );
        assert!(
            InviteKey::generate(&mut OsRng)
                .verifier()
                .verify_start_proof("seb", b"request", &proof)
                .is_err()
        );

        let proof = key.finish_proof("seb", b"sealed");
        assert_eq!(
            verifier.verify_finish_proof("seb", b"sealed", &proof),
            Ok(())
        );
        assert!(
            verifier
                .verify_finish_proof("seb", b"swapped", &proof)
                .is_err()
        );
        // A start proof is never a valid finish proof for the same bytes.
        assert!(
            verifier
                .verify_finish_proof("seb", b"request", &key.start_proof("seb", b"request"))
                .is_err()
        );
    }
}
