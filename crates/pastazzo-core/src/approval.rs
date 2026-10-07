//! Approving a new device.
//!
//! Logging in with the password isn't enough to open the account key: that
//! also takes the [`AccountSecret`], which only the account's devices have.
//! One of them hands it to the new device in a *grant*, sealed to the new
//! device's X25519 key, once the person approving has checked that both
//! devices show the same [`approval_code`].
//!
//! The code is what stops the server from slipping its own keys in: it's
//! computed from the new device's public keys, so if the approving device
//! were shown other keys, the codes wouldn't match. It's 64 bits, too many
//! for the server to search for keys that happen to give the same code in
//! the minutes an approval takes.
//!
//! Grants aren't signed: a forged one carries a wrong secret, and the
//! account key then fails to unwrap.

use zeroize::Zeroizing;

use crate::account::AccountSecret;
use crate::device::{DeviceKeys, DevicePublic};
use crate::wire::{Reader, check_version};
use crate::{Error, Id, PROTOCOL_VERSION, Rng, display_fingerprint};

/// What both devices show, to be compared before approving: the first 64
/// bits of the new device's fingerprint (`3F9A 12C4 5B7E 9D01`).
pub fn approval_code(device: &DevicePublic) -> String {
    display_fingerprint(&device.fingerprint()[..8])
}

/// The account secret sealed to a new device:
/// `version(1) || account(16) || device id(16) || secret(32)`.
pub fn seal_grant(
    secret: &AccountSecret,
    account: &Id,
    device: &DevicePublic,
    rng: &mut impl Rng,
) -> Vec<u8> {
    let mut plaintext = Zeroizing::new(Vec::with_capacity(65));
    plaintext.push(PROTOCOL_VERSION);
    plaintext.extend_from_slice(account);
    plaintext.extend_from_slice(&device.id);
    plaintext.extend_from_slice(secret.expose_secret());
    device.seal_to(&plaintext, rng)
}

/// Opens a grant addressed to this device for this account.
pub fn open_grant(keys: &DeviceKeys, account: &Id, sealed: &[u8]) -> Result<AccountSecret, Error> {
    let plaintext = keys.unseal(sealed)?;
    let mut reader = Reader::new(&plaintext, "approval grant");
    check_version(reader.u8()?, "approval grant")?;
    let for_account: Id = reader.array()?;
    let for_device: Id = reader.array()?;
    let secret = Zeroizing::new(reader.array::<32>()?);
    reader.finish()?;
    if for_account != *account || for_device != keys.id() {
        return Err(Error::Malformed("approval grant"));
    }
    Ok(AccountSecret::from_bytes(*secret))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{AccountKey, WrappedAccountKey};
    use rand::rngs::OsRng;

    #[test]
    fn an_approved_device_opens_the_account_key() {
        let key = AccountKey::generate(&mut OsRng);
        let secret = AccountSecret::generate(&mut OsRng);
        let wrapped = WrappedAccountKey::wrap(&key, &[9; 64], &secret, &[1; 16], &mut OsRng);

        let new_device = DeviceKeys::generate(&mut OsRng);
        let grant = seal_grant(&secret, &[1; 16], &new_device.public(), &mut OsRng);
        let received = open_grant(&new_device, &[1; 16], &grant).unwrap();
        let opened = wrapped.unwrap(&[9; 64], &received, &[1; 16]).unwrap();
        assert_eq!(opened.expose_secret(), key.expose_secret());
    }

    #[test]
    fn grants_only_work_for_their_device_and_account() {
        let secret = AccountSecret::generate(&mut OsRng);
        let device = DeviceKeys::generate(&mut OsRng);
        let grant = seal_grant(&secret, &[1; 16], &device.public(), &mut OsRng);
        assert!(open_grant(&device, &[2; 16], &grant).is_err());
        assert!(open_grant(&DeviceKeys::generate(&mut OsRng), &[1; 16], &grant).is_err());

        // Sealed to this device's key, but naming another device.
        let mut other = DeviceKeys::generate(&mut OsRng).public();
        other.exchange = device.public().exchange;
        let misaddressed = seal_grant(&secret, &[1; 16], &other, &mut OsRng);
        assert!(open_grant(&device, &[1; 16], &misaddressed).is_err());
    }

    #[test]
    fn a_forged_grant_doesnt_open_the_account_key() {
        let key = AccountKey::generate(&mut OsRng);
        let secret = AccountSecret::generate(&mut OsRng);
        let wrapped = WrappedAccountKey::wrap(&key, &[9; 64], &secret, &[1; 16], &mut OsRng);
        let device = DeviceKeys::generate(&mut OsRng);
        // Anyone can seal to the device's public key, just not the right secret.
        let forged = seal_grant(
            &AccountSecret::generate(&mut OsRng),
            &[1; 16],
            &device.public(),
            &mut OsRng,
        );
        let secret = open_grant(&device, &[1; 16], &forged).unwrap();
        assert_eq!(
            wrapped.unwrap(&[9; 64], &secret, &[1; 16]).err(),
            Some(Error::Decrypt)
        );
    }

    #[test]
    fn the_code_depends_on_the_keys() {
        let device = DeviceKeys::generate(&mut OsRng).public();
        let code = approval_code(&device);
        assert_eq!(code.len(), 19, "{code}");
        assert_eq!(code, approval_code(&device));
        let mut swapped = device;
        swapped.exchange = DeviceKeys::generate(&mut OsRng).public().exchange;
        assert_ne!(code, approval_code(&swapped));
    }
}
