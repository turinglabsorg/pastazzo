//! What the OPAQUE session key authenticates.
//!
//! A login ends with a session key that only the client and the server know,
//! not a relay in between. Two messages are bound to it:
//!
//! - the client's device public keys, so the server only accepts signatures
//!   from keys that really came from someone who knows the password;
//! - the server's answer (account id and wrapped account key), so the client
//!   knows it wasn't swapped.

use crate::account::WrappedAccountKey;
use crate::device::DevicePublic;
use crate::{Error, Id, derive_key, mac, transcript, verify_mac};

fn session_mac_key(session_key: &[u8], purpose: &str) -> zeroize::Zeroizing<[u8; 32]> {
    derive_key(
        session_key,
        &transcript("pastazzo/v1/session-mac", &[purpose.as_bytes()]),
    )
}

fn device_message(username: &str, device: &DevicePublic) -> Vec<u8> {
    transcript(
        "pastazzo/v1/device-binding",
        &[username.as_bytes(), &device.to_bytes()],
    )
}

fn login_response_message(username: &str, account: &Id, wrapped: &WrappedAccountKey) -> Vec<u8> {
    transcript(
        "pastazzo/v1/login-response",
        &[username.as_bytes(), account, &wrapped.to_bytes()],
    )
}

/// Client: tag for the device keys sent with the last login message.
pub fn device_binding_tag(session_key: &[u8], username: &str, device: &DevicePublic) -> [u8; 32] {
    mac(
        session_mac_key(session_key, "device-binding").as_ref(),
        &device_message(username, device),
    )
}

/// Server: checks the device keys came with this login.
pub fn verify_device_binding(
    session_key: &[u8],
    username: &str,
    device: &DevicePublic,
    tag: &[u8],
) -> Result<(), Error> {
    verify_mac(
        session_mac_key(session_key, "device-binding").as_ref(),
        &device_message(username, device),
        tag,
    )
}

/// Server: tag for the login response.
pub fn login_response_tag(
    session_key: &[u8],
    username: &str,
    account: &Id,
    wrapped: &WrappedAccountKey,
) -> [u8; 32] {
    mac(
        session_mac_key(session_key, "login-response").as_ref(),
        &login_response_message(username, account, wrapped),
    )
}

/// Client: checks the login response came from the server it logged in to.
pub fn verify_login_response(
    session_key: &[u8],
    username: &str,
    account: &Id,
    wrapped: &WrappedAccountKey,
    tag: &[u8],
) -> Result<(), Error> {
    verify_mac(
        session_mac_key(session_key, "login-response").as_ref(),
        &login_response_message(username, account, wrapped),
        tag,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::AccountKey;
    use crate::device::DeviceKeys;
    use rand::rngs::OsRng;

    #[test]
    fn device_binding() {
        let device = DeviceKeys::generate(&mut OsRng).public();
        let tag = device_binding_tag(b"session", "seb", &device);
        assert_eq!(
            verify_device_binding(b"session", "seb", &device, &tag),
            Ok(())
        );
        assert!(verify_device_binding(b"other session", "seb", &device, &tag).is_err());
        assert!(verify_device_binding(b"session", "other", &device, &tag).is_err());
        let swapped = DeviceKeys::generate(&mut OsRng).public();
        assert!(verify_device_binding(b"session", "seb", &swapped, &tag).is_err());
    }

    #[test]
    fn login_response() {
        let wrapped = WrappedAccountKey::wrap(
            &AccountKey::generate(&mut OsRng),
            &[1; 64],
            &[2; 16],
            &mut OsRng,
        );
        let tag = login_response_tag(b"session", "seb", &[2; 16], &wrapped);
        assert_eq!(
            verify_login_response(b"session", "seb", &[2; 16], &wrapped, &tag),
            Ok(())
        );
        assert!(verify_login_response(b"session", "seb", &[3; 16], &wrapped, &tag).is_err());
        let other = WrappedAccountKey::wrap(
            &AccountKey::generate(&mut OsRng),
            &[1; 64],
            &[2; 16],
            &mut OsRng,
        );
        assert!(verify_login_response(b"session", "seb", &[2; 16], &other, &tag).is_err());
        // The two tags use different keys: one can't stand in for the other.
        let device = DeviceKeys::generate(&mut OsRng).public();
        assert_ne!(
            device_binding_tag(b"session", "seb", &device),
            login_response_tag(b"session", "seb", &[2; 16], &wrapped)
        );
    }
}
