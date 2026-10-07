//! Device keys and device records.
//!
//! Every device has its own Ed25519 key, used to sign requests so the server
//! knows which device is talking without bearer tokens a relay could steal,
//! and an X25519 key, for receiving a rotated account key sealed to it.
//!
//! The list of devices is kept by the server, but each entry is a
//! [`SealedDeviceRecord`]: the device's public keys authenticated, and its
//! name encrypted, with the account key's `devices` subkey. A server can hide
//! a device, but can't add one that other devices would accept.

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use zeroize::Zeroizing;

use crate::account::AccountKey;
use crate::pad::{MIN_PADDED_LEN, pad, unpad};
use crate::wire::{Reader, check_version, push_bytes};
use crate::{Error, Id, PROTOCOL_VERSION, Rng, aead_open, aead_seal, random_id, transcript};

const MAX_NAME_LEN: usize = 128;

/// A device's secret keys. Stored in the OS keychain, never sent anywhere.
pub struct DeviceKeys {
    id: Id,
    signing: SigningKey,
    exchange: crypto_box::SecretKey,
}

impl DeviceKeys {
    pub fn generate(rng: &mut impl Rng) -> Self {
        Self {
            id: random_id(rng),
            signing: SigningKey::generate(rng),
            exchange: crypto_box::SecretKey::generate(rng),
        }
    }

    pub fn id(&self) -> Id {
        self.id
    }

    pub fn public(&self) -> DevicePublic {
        DevicePublic {
            id: self.id,
            signing: self.signing.verifying_key().to_bytes(),
            exchange: self.exchange.public_key().to_bytes(),
        }
    }

    pub(crate) fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing.sign(message).to_bytes()
    }

    /// Opens a sealed box addressed to this device.
    pub fn unseal(&self, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
        self.exchange
            .unseal(sealed)
            .map(Zeroizing::new)
            .map_err(|_| Error::Decrypt)
    }

    /// `version(1) || id(16) || ed25519 secret(32) || x25519 secret(32)`,
    /// for the OS keychain.
    pub fn to_secret_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(81));
        out.push(PROTOCOL_VERSION);
        out.extend_from_slice(&self.id);
        out.extend_from_slice(self.signing.as_bytes());
        out.extend_from_slice(&self.exchange.to_bytes());
        out
    }

    pub fn from_secret_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes, "device keys");
        check_version(reader.u8()?, "device keys")?;
        let id = reader.array()?;
        let signing = Zeroizing::new(reader.array::<32>()?);
        let exchange = Zeroizing::new(reader.array::<32>()?);
        reader.finish()?;
        Ok(Self {
            id,
            signing: SigningKey::from_bytes(&signing),
            exchange: crypto_box::SecretKey::from_bytes(*exchange),
        })
    }
}

/// A device's public keys, as registered with the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevicePublic {
    pub id: Id,
    /// Ed25519 verifying key.
    pub signing: [u8; 32],
    /// X25519 public key.
    pub exchange: [u8; 32],
}

impl DevicePublic {
    /// `id(16) || signing(32) || exchange(32)`
    pub fn to_bytes(&self) -> [u8; 80] {
        let mut out = [0u8; 80];
        out[..16].copy_from_slice(&self.id);
        out[16..48].copy_from_slice(&self.signing);
        out[48..].copy_from_slice(&self.exchange);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes, "device public keys");
        let public = Self {
            id: reader.array()?,
            signing: reader.array()?,
            exchange: reader.array()?,
        };
        reader.finish()?;
        public.verifying_key()?;
        Ok(public)
    }

    /// SHA-256 of the device's public keys, to compare across devices.
    pub fn fingerprint(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        Sha256::digest(transcript(
            "pastazzo/v1/device-fingerprint",
            &[&self.to_bytes()],
        ))
        .into()
    }

    pub(crate) fn verifying_key(&self) -> Result<VerifyingKey, Error> {
        VerifyingKey::from_bytes(&self.signing).map_err(|_| Error::Malformed("device signing key"))
    }

    /// Seals `plaintext` so only this device can open it (libsodium sealed
    /// box). Anonymous: whoever uses it must authenticate the content.
    pub fn seal_to(&self, plaintext: &[u8], rng: &mut impl Rng) -> Vec<u8> {
        crypto_box::PublicKey::from_bytes(self.exchange)
            .seal(rng, plaintext)
            .expect("sealing an in-memory buffer cannot fail")
    }
}

/// A device entry as other devices check it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedDeviceRecord {
    pub device: DevicePublic,
    pub epoch: u32,
    pub nonce: [u8; 24],
    /// The device name, encrypted.
    pub ciphertext: Vec<u8>,
}

impl SealedDeviceRecord {
    pub fn seal(
        key: &AccountKey,
        account: &Id,
        device: DevicePublic,
        name: &str,
        rng: &mut impl Rng,
    ) -> Result<Self, Error> {
        if name.len() > MAX_NAME_LEN {
            return Err(Error::TooLarge);
        }
        let (nonce, ciphertext) = aead_seal(
            &key.devices_key(),
            &pad(name.as_bytes())?,
            &record_aad(account, &device, key.epoch()),
            rng,
        );
        Ok(Self {
            device,
            epoch: key.epoch(),
            nonce,
            ciphertext,
        })
    }

    /// Checks the record was made by a holder of the account key and returns
    /// the device name.
    pub fn open(&self, key: &AccountKey, account: &Id) -> Result<String, Error> {
        if self.epoch != key.epoch() {
            return Err(Error::Decrypt);
        }
        let padded = aead_open(
            &key.devices_key(),
            &self.nonce,
            &self.ciphertext,
            &record_aad(account, &self.device, self.epoch),
        )?;
        let name = unpad(&padded)?;
        if name.len() > MAX_NAME_LEN {
            return Err(Error::TooLarge);
        }
        String::from_utf8(name.to_vec()).map_err(|_| Error::Malformed("device name"))
    }

    /// `version(1) || device(80) || u32be(epoch) || nonce(24) || u32be(len) || ciphertext`
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 80 + 4 + 24 + 4 + self.ciphertext.len());
        out.push(PROTOCOL_VERSION);
        out.extend_from_slice(&self.device.to_bytes());
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&self.nonce);
        push_bytes(&mut out, &self.ciphertext);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes, "device record");
        check_version(reader.u8()?, "device record")?;
        let device = DevicePublic::from_bytes(&reader.array::<80>()?)?;
        let epoch = reader.u32()?;
        let nonce = reader.array()?;
        let ciphertext = reader.bytes()?.to_vec();
        reader.finish()?;
        // Names are padded to a single block, so every record is the same size.
        if ciphertext.len() != MIN_PADDED_LEN + 16 {
            return Err(Error::Malformed("device record"));
        }
        Ok(Self {
            device,
            epoch,
            nonce,
            ciphertext,
        })
    }
}

fn record_aad(account: &Id, device: &DevicePublic, epoch: u32) -> Vec<u8> {
    transcript(
        "pastazzo/v1/device-record",
        &[account, &device.to_bytes(), &epoch.to_be_bytes()],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn keys_roundtrip_through_the_keychain_format() {
        let keys = DeviceKeys::generate(&mut OsRng);
        let restored = DeviceKeys::from_secret_bytes(&keys.to_secret_bytes()).unwrap();
        assert_eq!(restored.public(), keys.public());
        assert_eq!(
            DevicePublic::from_bytes(&keys.public().to_bytes()).unwrap(),
            keys.public()
        );
    }

    #[test]
    fn record_roundtrip() {
        let key = AccountKey::generate(&mut OsRng);
        let device = DeviceKeys::generate(&mut OsRng).public();
        let record =
            SealedDeviceRecord::seal(&key, &[1; 16], device, "Mac Pro", &mut OsRng).unwrap();
        // Fixed size whatever the name: it doesn't leak its length.
        assert_eq!(record.to_bytes().len(), 385);
        let decoded = SealedDeviceRecord::from_bytes(&record.to_bytes()).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(decoded.open(&key, &[1; 16]).unwrap(), "Mac Pro");
    }

    #[test]
    fn server_cannot_swap_device_keys() {
        let key = AccountKey::generate(&mut OsRng);
        let record = SealedDeviceRecord::seal(
            &key,
            &[1; 16],
            DeviceKeys::generate(&mut OsRng).public(),
            "phone",
            &mut OsRng,
        )
        .unwrap();
        let mut swapped = record.clone();
        swapped.device = DeviceKeys::generate(&mut OsRng).public();
        assert_eq!(swapped.open(&key, &[1; 16]).err(), Some(Error::Decrypt));
        assert_eq!(record.open(&key, &[2; 16]).err(), Some(Error::Decrypt));
        assert_eq!(
            record
                .open(&AccountKey::generate(&mut OsRng), &[1; 16])
                .err(),
            Some(Error::Decrypt)
        );
    }

    #[test]
    fn sealed_box_to_device() {
        let keys = DeviceKeys::generate(&mut OsRng);
        let sealed = keys.public().seal_to(b"new account key", &mut OsRng);
        assert_eq!(keys.unseal(&sealed).unwrap().as_slice(), b"new account key");
        assert!(DeviceKeys::generate(&mut OsRng).unseal(&sealed).is_err());
    }
}
