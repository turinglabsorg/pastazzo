//! Signed requests.
//!
//! After login a device authenticates every request with its Ed25519 key.
//! The signature covers the server's fingerprint, the account, the method,
//! path and query, a timestamp, a random nonce and the SHA-256 of the body.
//! A relay that sees the request can't change it, reuse the signature for
//! another request, server or account, or replay it once the server has seen
//! the nonce or the timestamp has gone stale.

use ed25519_dalek::Signature;
use sha2::{Digest, Sha256};

use crate::device::{DeviceKeys, DevicePublic};
use crate::{Error, Id, Rng, transcript};

/// How far a request timestamp may be from the server clock. The server must
/// remember nonces for at least twice this long.
pub const MAX_CLOCK_SKEW_MS: u64 = 5 * 60 * 1000;

/// What a request is signed for: which server, which account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scope {
    pub server_fingerprint: [u8; 32],
    pub account: Id,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestSignature {
    pub device: Id,
    /// Milliseconds since the Unix epoch.
    pub timestamp: u64,
    pub nonce: [u8; 16],
    pub signature: [u8; 64],
}

impl RequestSignature {
    pub fn sign(
        keys: &DeviceKeys,
        scope: &Scope,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        timestamp: u64,
        rng: &mut impl Rng,
    ) -> Self {
        let mut nonce = [0u8; 16];
        rng.fill_bytes(&mut nonce);
        Self::sign_with_nonce(keys, scope, method, path_and_query, body, timestamp, nonce)
    }

    pub(crate) fn sign_with_nonce(
        keys: &DeviceKeys,
        scope: &Scope,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        timestamp: u64,
        nonce: [u8; 16],
    ) -> Self {
        let device = keys.id();
        let message = signed_message(
            scope,
            &device,
            method,
            path_and_query,
            body,
            timestamp,
            &nonce,
        );
        Self {
            device,
            timestamp,
            nonce,
            signature: keys.sign(&message),
        }
    }

    /// Checks the signature and the timestamp. Replay protection also needs
    /// the caller to reject a `(device, nonce)` pair it has already seen.
    pub fn verify(
        &self,
        device: &DevicePublic,
        scope: &Scope,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        now: u64,
    ) -> Result<(), Error> {
        if device.id != self.device {
            return Err(Error::BadSignature);
        }
        if now.abs_diff(self.timestamp) > MAX_CLOCK_SKEW_MS {
            return Err(Error::StaleRequest);
        }
        let message = signed_message(
            scope,
            &self.device,
            method,
            path_and_query,
            body,
            self.timestamp,
            &self.nonce,
        );
        device
            .verifying_key()?
            .verify_strict(&message, &Signature::from_bytes(&self.signature))
            .map_err(|_| Error::BadSignature)
    }
}

fn signed_message(
    scope: &Scope,
    device: &Id,
    method: &str,
    path_and_query: &str,
    body: &[u8],
    timestamp: u64,
    nonce: &[u8; 16],
) -> Vec<u8> {
    transcript(
        "pastazzo/v1/request",
        &[
            &scope.server_fingerprint,
            &scope.account,
            device,
            method.as_bytes(),
            path_and_query.as_bytes(),
            &timestamp.to_be_bytes(),
            nonce,
            &Sha256::digest(body),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    const NOW: u64 = 1_791_379_434_274;
    const SCOPE: Scope = Scope {
        server_fingerprint: [3; 32],
        account: [4; 16],
    };

    fn sign(keys: &DeviceKeys, method: &str, path: &str, body: &[u8]) -> RequestSignature {
        RequestSignature::sign(keys, &SCOPE, method, path, body, NOW, &mut OsRng)
    }

    #[test]
    fn valid_request_verifies() {
        let keys = DeviceKeys::generate(&mut OsRng);
        let sig = sign(&keys, "POST", "/v1/items", b"body");
        assert_eq!(
            sig.verify(
                &keys.public(),
                &SCOPE,
                "POST",
                "/v1/items",
                b"body",
                NOW + 1000
            ),
            Ok(())
        );
    }

    #[test]
    fn any_change_breaks_the_signature() {
        let keys = DeviceKeys::generate(&mut OsRng);
        let device = keys.public();
        let sig = sign(&keys, "POST", "/v1/items", b"body");
        let verify = |sig: &RequestSignature, scope: &Scope, method, path, body: &[u8]| {
            sig.verify(&device, scope, method, path, body, NOW)
        };
        assert_eq!(
            verify(&sig, &SCOPE, "PUT", "/v1/items", b"body"),
            Err(Error::BadSignature)
        );
        assert_eq!(
            verify(&sig, &SCOPE, "POST", "/v1/items?x=1", b"body"),
            Err(Error::BadSignature)
        );
        assert_eq!(
            verify(&sig, &SCOPE, "POST", "/v1/items", b"bodx"),
            Err(Error::BadSignature)
        );

        let other_server = Scope {
            server_fingerprint: [9; 32],
            ..SCOPE
        };
        assert_eq!(
            verify(&sig, &other_server, "POST", "/v1/items", b"body"),
            Err(Error::BadSignature)
        );
        let other_account = Scope {
            account: [9; 16],
            ..SCOPE
        };
        assert_eq!(
            verify(&sig, &other_account, "POST", "/v1/items", b"body"),
            Err(Error::BadSignature)
        );

        let mut other_nonce = sig;
        other_nonce.nonce[0] ^= 1;
        assert_eq!(
            verify(&other_nonce, &SCOPE, "POST", "/v1/items", b"body"),
            Err(Error::BadSignature)
        );

        let mut other_time = sig;
        other_time.timestamp += 1;
        assert_eq!(
            verify(&other_time, &SCOPE, "POST", "/v1/items", b"body"),
            Err(Error::BadSignature)
        );
    }

    #[test]
    fn other_devices_and_stale_requests_are_rejected() {
        let keys = DeviceKeys::generate(&mut OsRng);
        let sig = sign(&keys, "GET", "/v1/stream", b"");
        let other = DeviceKeys::generate(&mut OsRng).public();
        assert_eq!(
            sig.verify(&other, &SCOPE, "GET", "/v1/stream", b"", NOW),
            Err(Error::BadSignature)
        );
        let mut impostor = other;
        impostor.id = keys.id();
        assert_eq!(
            sig.verify(&impostor, &SCOPE, "GET", "/v1/stream", b"", NOW),
            Err(Error::BadSignature)
        );
        for now in [NOW + MAX_CLOCK_SKEW_MS + 1, NOW - MAX_CLOCK_SKEW_MS - 1] {
            assert_eq!(
                sig.verify(&keys.public(), &SCOPE, "GET", "/v1/stream", b"", now),
                Err(Error::StaleRequest)
            );
        }
    }
}
