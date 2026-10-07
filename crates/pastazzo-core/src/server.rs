//! Server identity.
//!
//! A server has three key pairs:
//!
//! - OPAQUE (Ristretto255), which proves its identity at every login;
//! - X25519 transport, which clients seal the registration record to, so a
//!   relay never sees it;
//! - Ed25519 signing, which signs the registration response, so a relay
//!   can't replace it with one computed from its own OPRF key.
//!
//! Clients pin the [`ServerIdentity::fingerprint`] of all three, from the
//! invite link or on first use.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

use crate::opaque::Suite;
use crate::wire::{Reader, check_version, push_bytes};
use crate::{Error, PROTOCOL_VERSION, Rng, transcript};

/// The server's secret keys.
pub struct ServerKeys {
    opaque: opaque_ke::ServerSetup<Suite>,
    transport: crypto_box::SecretKey,
    signing: SigningKey,
}

impl ServerKeys {
    pub fn generate(rng: &mut impl Rng) -> Self {
        Self {
            opaque: opaque_ke::ServerSetup::new(rng),
            transport: crypto_box::SecretKey::generate(rng),
            signing: SigningKey::generate(rng),
        }
    }

    pub fn identity(&self) -> ServerIdentity {
        let opaque = self.opaque.keypair().public().serialize();
        ServerIdentity {
            opaque: opaque
                .as_slice()
                .try_into()
                .expect("Ristretto255 public keys are 32 bytes"),
            transport: self.transport.public_key().to_bytes(),
            signing: self.signing.verifying_key().to_bytes(),
        }
    }

    pub(crate) fn opaque(&self) -> &opaque_ke::ServerSetup<Suite> {
        &self.opaque
    }

    pub(crate) fn unseal(&self, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
        self.transport
            .unseal(sealed)
            .map(Zeroizing::new)
            .map_err(|_| Error::Decrypt)
    }

    pub(crate) fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing.sign(message).to_bytes()
    }

    /// `version(1) || u32be(len) || OPAQUE server setup || x25519 secret(32) || ed25519 secret(32)`
    pub fn to_secret_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::new());
        out.push(PROTOCOL_VERSION);
        let mut setup = self.opaque.serialize();
        push_bytes(&mut out, &setup);
        setup.as_mut_slice().zeroize();
        out.extend_from_slice(&Zeroizing::new(self.transport.to_bytes())[..]);
        out.extend_from_slice(self.signing.as_bytes());
        out
    }

    pub fn from_secret_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes, "server keys");
        check_version(reader.u8()?, "server keys")?;
        let opaque = opaque_ke::ServerSetup::deserialize(reader.bytes()?)
            .map_err(|_| Error::Malformed("server keys"))?;
        let transport = Zeroizing::new(reader.array::<32>()?);
        let signing = Zeroizing::new(reader.array::<32>()?);
        reader.finish()?;
        Ok(Self {
            opaque,
            transport: crypto_box::SecretKey::from_bytes(*transport),
            signing: SigningKey::from_bytes(&signing),
        })
    }
}

/// The server's public keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerIdentity {
    /// OPAQUE (Ristretto255) public key.
    pub opaque: [u8; 32],
    /// X25519 public key for sealing the registration record.
    pub transport: [u8; 32],
    /// Ed25519 public key for the registration response.
    pub signing: [u8; 32],
}

impl ServerIdentity {
    /// SHA-256 of the three public keys. This is what clients pin.
    pub fn fingerprint(&self) -> [u8; 32] {
        Sha256::digest(transcript(
            "pastazzo/v1/server-identity",
            &[&self.opaque, &self.transport, &self.signing],
        ))
        .into()
    }

    /// Checks these are the keys a pinned fingerprint was made from.
    pub fn verify(&self, pinned: &[u8; 32]) -> Result<(), Error> {
        if bool::from(self.fingerprint().ct_eq(pinned)) {
            Ok(())
        } else {
            Err(Error::UnknownServer)
        }
    }

    pub(crate) fn seal(&self, plaintext: &[u8], rng: &mut impl Rng) -> Vec<u8> {
        crypto_box::PublicKey::from_bytes(self.transport)
            .seal(rng, plaintext)
            .expect("sealing an in-memory buffer cannot fail")
    }

    pub(crate) fn verify_signature(
        &self,
        message: &[u8],
        signature: &[u8; 64],
    ) -> Result<(), Error> {
        VerifyingKey::from_bytes(&self.signing)
            .map_err(|_| Error::Malformed("server signing key"))?
            .verify_strict(message, &Signature::from_bytes(signature))
            .map_err(|_| Error::BadSignature)
    }

    /// `opaque(32) || transport(32) || signing(32)`
    pub fn to_bytes(&self) -> [u8; 96] {
        let mut out = [0u8; 96];
        out[..32].copy_from_slice(&self.opaque);
        out[32..64].copy_from_slice(&self.transport);
        out[64..].copy_from_slice(&self.signing);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes, "server identity");
        let identity = Self {
            opaque: reader.array()?,
            transport: reader.array()?,
            signing: reader.array()?,
        };
        reader.finish()?;
        Ok(identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn keys_roundtrip_and_identity_is_stable() {
        let keys = ServerKeys::generate(&mut OsRng);
        let restored = ServerKeys::from_secret_bytes(&keys.to_secret_bytes()).unwrap();
        assert_eq!(restored.identity(), keys.identity());
        assert_eq!(restored.sign(b"m"), keys.sign(b"m"));
        assert_eq!(
            ServerIdentity::from_bytes(&keys.identity().to_bytes()).unwrap(),
            keys.identity()
        );
    }

    #[test]
    fn fingerprint_pins_all_keys() {
        let identity = ServerKeys::generate(&mut OsRng).identity();
        let pinned = identity.fingerprint();
        assert_eq!(identity.verify(&pinned), Ok(()));

        let other = ServerKeys::generate(&mut OsRng).identity();
        for swap in [
            ServerIdentity {
                opaque: other.opaque,
                ..identity
            },
            ServerIdentity {
                transport: other.transport,
                ..identity
            },
            ServerIdentity {
                signing: other.signing,
                ..identity
            },
        ] {
            assert_eq!(swap.verify(&pinned), Err(Error::UnknownServer));
        }
    }

    #[test]
    fn only_the_server_opens_what_is_sealed_to_it() {
        let keys = ServerKeys::generate(&mut OsRng);
        let sealed = keys.identity().seal(b"record", &mut OsRng);
        assert_eq!(keys.unseal(&sealed).unwrap().as_slice(), b"record");
        assert!(ServerKeys::generate(&mut OsRng).unseal(&sealed).is_err());
    }

    #[test]
    fn signatures() {
        let keys = ServerKeys::generate(&mut OsRng);
        let signature = keys.sign(b"response");
        assert_eq!(
            keys.identity().verify_signature(b"response", &signature),
            Ok(())
        );
        assert_eq!(
            keys.identity().verify_signature(b"other", &signature),
            Err(Error::BadSignature)
        );
        let other = ServerKeys::generate(&mut OsRng).identity();
        assert_eq!(
            other.verify_signature(b"response", &signature),
            Err(Error::BadSignature)
        );
    }
}
