use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Decryption failed: wrong key, or the data was tampered with or forged.
    Decrypt,
    /// A MAC didn't verify.
    BadMac,
    /// A signature didn't verify.
    BadSignature,
    /// A signed request is outside the accepted clock window.
    StaleRequest,
    /// Data that doesn't follow the protocol encoding.
    Malformed(&'static str),
    /// Content over the protocol size limits.
    TooLarge,
    /// OPAQUE failed: wrong password, an invalid message, or a server that
    /// isn't the one we pinned.
    Opaque,
    /// The server's identity doesn't match the pinned fingerprint.
    UnknownServer,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Decrypt => write!(f, "decryption failed"),
            Error::BadMac => write!(f, "authentication tag mismatch"),
            Error::BadSignature => write!(f, "invalid signature"),
            Error::StaleRequest => write!(f, "request timestamp outside the accepted window"),
            Error::Malformed(what) => write!(f, "malformed {what}"),
            Error::TooLarge => write!(f, "content over the size limit"),
            Error::Opaque => write!(f, "password authentication failed"),
            Error::UnknownServer => {
                write!(f, "server identity doesn't match the pinned fingerprint")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<opaque_ke::errors::ProtocolError> for Error {
    fn from(_: opaque_ke::errors::ProtocolError) -> Self {
        Error::Opaque
    }
}
