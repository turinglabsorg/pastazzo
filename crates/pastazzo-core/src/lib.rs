//! Protocol and end-to-end encryption shared by every pastazzo client and
//! server.
//!
//! Nothing in here trusts the network, TLS or the server: clipboard contents,
//! the account key and the password never leave a device unencrypted. See
//! `docs/SECURITY.md` for the threat model and `docs/PROTOCOL.md` for the
//! wire format.

pub mod account;
pub mod api;
pub mod device;
mod error;
pub mod invite;
pub mod item;
pub mod opaque;
pub mod pad;
pub mod registration;
pub mod request;
pub mod server;
pub mod session;
mod transcript;
#[cfg(test)]
mod vectors;
mod wire;

pub use error::Error;
pub use transcript::transcript;

/// Version of the protocol implemented by this crate, bound into every
/// derived key and every authenticated message.
pub const PROTOCOL_VERSION: u8 = 1;

/// Random identifier for accounts, devices, items and invites.
pub type Id = [u8; 16];

/// A cryptographically secure random number generator, such as
/// `rand::rngs::OsRng`.
pub trait Rng: rand::RngCore + rand::CryptoRng {}
impl<T: rand::RngCore + rand::CryptoRng> Rng for T {}

/// Generates a random [`Id`].
pub fn random_id(rng: &mut impl Rng) -> Id {
    let mut id = [0u8; 16];
    rng.fill_bytes(&mut id);
    id
}

/// HKDF-SHA256 with an empty salt, the only key derivation used outside
/// OPAQUE. `info` is always a [`transcript`] so labels can't collide.
pub(crate) fn derive_key(ikm: &[u8], info: &[u8]) -> zeroize::Zeroizing<[u8; 32]> {
    let mut okm = zeroize::Zeroizing::new([0u8; 32]);
    hkdf::Hkdf::<sha2::Sha256>::new(None, ikm)
        .expand(info, okm.as_mut())
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    okm
}

/// XChaCha20-Poly1305 encryption with a fresh random nonce.
pub(crate) fn aead_seal(
    key: &[u8; 32],
    plaintext: &[u8],
    aad: &[u8],
    rng: &mut impl Rng,
) -> ([u8; 24], Vec<u8>) {
    let mut nonce = [0u8; 24];
    rng.fill_bytes(&mut nonce);
    let ciphertext = aead_seal_with_nonce(key, &nonce, plaintext, aad);
    (nonce, ciphertext)
}

/// XChaCha20-Poly1305 encryption with a caller-chosen nonce. Only for test
/// vectors: a nonce must never be reused with the same key.
pub(crate) fn aead_seal_with_nonce(
    key: &[u8; 32],
    nonce: &[u8; 24],
    plaintext: &[u8],
    aad: &[u8],
) -> Vec<u8> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    chacha20poly1305::XChaCha20Poly1305::new(key.into())
        .encrypt(
            nonce.into(),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("XChaCha20-Poly1305 encryption cannot fail for in-memory buffers")
}

pub(crate) fn aead_open(
    key: &[u8; 32],
    nonce: &[u8; 24],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, Error> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    chacha20poly1305::XChaCha20Poly1305::new(key.into())
        .decrypt(
            nonce.into(),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| Error::Decrypt)
}

/// HMAC-SHA256.
pub(crate) fn mac(key: &[u8], message: &[u8]) -> [u8; 32] {
    use hmac::Mac;
    let mut mac =
        hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(message);
    mac.finalize().into_bytes().into()
}

/// Constant-time HMAC-SHA256 verification.
pub(crate) fn verify_mac(key: &[u8], message: &[u8], tag: &[u8]) -> Result<(), Error> {
    use hmac::Mac;
    let mut mac =
        hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(message);
    mac.verify_slice(tag).map_err(|_| Error::BadMac)
}
