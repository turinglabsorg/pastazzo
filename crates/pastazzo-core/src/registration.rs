//! The registration record.
//!
//! The last registration message carries the OPAQUE record, the new account
//! id and the wrapped account key. OPAQUE's record is about as sensitive as a
//! password hash, so the whole message is sealed to the server's transport
//! key: a relay sees only ciphertext.

use zeroize::Zeroizing;

use crate::account::WrappedAccountKey;
use crate::server::{ServerIdentity, ServerKeys};
use crate::wire::{Reader, check_version, push_bytes};
use crate::{Error, Id, PROTOCOL_VERSION, Rng};

pub struct RegistrationRecord {
    pub username: String,
    pub account: Id,
    /// OPAQUE registration upload.
    pub upload: Zeroizing<Vec<u8>>,
    pub wrapped: WrappedAccountKey,
}

impl RegistrationRecord {
    /// Client: seal for the pinned server.
    pub fn seal(&self, server: &ServerIdentity, rng: &mut impl Rng) -> Vec<u8> {
        server.seal(&self.to_bytes(), rng)
    }

    /// Server: open a sealed record.
    pub fn open(keys: &ServerKeys, sealed: &[u8]) -> Result<Self, Error> {
        Self::from_bytes(&keys.unseal(sealed)?)
    }

    /// `version(1) || u32be(len) || username || account(16) || u32be(len) || upload || u32be(len) || wrapped`
    fn to_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(vec![PROTOCOL_VERSION]);
        push_bytes(&mut out, self.username.as_bytes());
        out.extend_from_slice(&self.account);
        push_bytes(&mut out, &self.upload);
        push_bytes(&mut out, &self.wrapped.to_bytes());
        out
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes, "registration record");
        check_version(reader.u8()?, "registration record")?;
        let username = String::from_utf8(reader.bytes()?.to_vec())
            .map_err(|_| Error::Malformed("username"))?;
        crate::opaque::validate_username(&username)?;
        let account = reader.array()?;
        let upload = Zeroizing::new(reader.bytes()?.to_vec());
        let wrapped = WrappedAccountKey::from_bytes(reader.bytes()?)?;
        reader.finish()?;
        Ok(Self {
            username,
            account,
            upload,
            wrapped,
        })
    }
}
