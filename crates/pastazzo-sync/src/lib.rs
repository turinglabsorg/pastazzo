//! pastazzo sync client: joins or logs in to a pastazzo server and keeps the
//! local clipboard in sync with the account's other devices. Everything that
//! leaves the device is end-to-end encrypted by `pastazzo-core`.

pub mod account;
pub mod clipboard;
pub mod daemon;
pub mod remote;
pub mod secrets;
pub mod state;

use std::time::{SystemTime, UNIX_EPOCH};

pub type Result<T> = std::result::Result<T, String>;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_millis() as u64
}

/// Log a line to stderr, which launchd and systemd keep.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { eprintln!("pastazzo-sync: {}", format_args!($($arg)*)) };
}
