//! JSON bodies and headers of the HTTP API, shared by the server and the
//! clients. Binary values travel as base64url without padding. Item uploads
//! and device records are binary bodies, not JSON.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Signed-request headers (see [`crate::request`]), all base64url except the
/// timestamp, which is decimal milliseconds.
pub const HEADER_DEVICE: &str = "pastazzo-device";
pub const HEADER_TIMESTAMP: &str = "pastazzo-timestamp";
pub const HEADER_NONCE: &str = "pastazzo-nonce";
pub const HEADER_SIGNATURE: &str = "pastazzo-signature";

/// How long `GET /v1/items` may hold a request open waiting for new items.
pub const MAX_WAIT_SECONDS: u64 = 30;

/// Bytes, as base64url without padding in JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct B64(pub Vec<u8>);

impl B64 {
    pub fn encode(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }

    pub fn decode(text: &str) -> Option<Vec<u8>> {
        URL_SAFE_NO_PAD.decode(text).ok()
    }

    /// Exactly `N` bytes, or `None`.
    pub fn array<const N: usize>(&self) -> Option<[u8; N]> {
        self.0.as_slice().try_into().ok()
    }
}

impl From<&[u8]> for B64 {
    fn from(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}

impl From<Vec<u8>> for B64 {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl Serialize for B64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&Self::encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for B64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::decode(&text)
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom("invalid base64url"))
    }
}

/// `GET /v1/server`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerInfo {
    pub version: u8,
    /// [`crate::server::ServerIdentity::to_bytes`]
    pub identity: B64,
    pub registration: Registration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Registration {
    Invite,
    Open,
    Closed,
}

/// `POST /v1/register/start`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterStart {
    pub username: String,
    pub request: B64,
    pub invite_id: Option<B64>,
    pub proof: Option<B64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterStarted {
    pub registration_id: B64,
    pub response: B64,
    pub signature: B64,
}

/// `POST /v1/register/finish`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterFinish {
    pub registration_id: B64,
    /// [`crate::registration::RegistrationRecord::seal`]
    pub sealed: B64,
    pub proof: Option<B64>,
}

/// `POST /v1/login/start`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginStart {
    pub username: String,
    pub request: B64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginStarted {
    pub login_id: B64,
    pub response: B64,
}

/// `POST /v1/login/finish`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginFinish {
    pub login_id: B64,
    pub finalization: B64,
    /// [`crate::device::DevicePublic::to_bytes`]
    pub device: B64,
    pub binding: B64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginFinished {
    pub account: B64,
    /// [`crate::account::WrappedAccountKey::to_bytes`]
    pub wrapped: B64,
    pub tag: B64,
}

/// `POST /v1/items` answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemPosted {
    pub cursor: u64,
}

/// `GET /v1/items?after=<cursor>&wait=<seconds>`: items after the cursor,
/// oldest first, and the cursor to ask from next.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemsPage {
    pub cursor: u64,
    /// [`crate::item::SealedItem::to_bytes`]
    pub items: Vec<B64>,
}

/// `GET /v1/devices`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceRecords {
    /// [`crate::device::SealedDeviceRecord::to_bytes`]
    pub records: Vec<B64>,
}

/// Body of every error response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_is_base64url_without_padding() {
        let json = serde_json::to_string(&B64(vec![0xfb, 0xff, 0x00])).unwrap();
        assert_eq!(json, "\"-_8A\"");
        assert_eq!(
            serde_json::from_str::<B64>(&json).unwrap().0,
            vec![0xfb, 0xff, 0x00]
        );
        assert!(serde_json::from_str::<B64>("\"-_8A=\"").is_err());
        assert!(serde_json::from_str::<B64>("\"+/8A\"").is_err());
        assert_eq!(B64(vec![1; 16]).array::<16>(), Some([1; 16]));
        assert_eq!(B64(vec![1; 15]).array::<16>(), None);
    }

    #[test]
    fn registration_mode_names() {
        assert_eq!(
            serde_json::to_string(&Registration::Invite).unwrap(),
            "\"invite\""
        );
        assert_eq!(
            serde_json::from_str::<Registration>("\"open\"").unwrap(),
            Registration::Open
        );
    }
}
