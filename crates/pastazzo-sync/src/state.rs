//! What a logged-in device keeps: the server, the account key and its own
//! keys. It's a secret, stored in a file only the owner can read.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use pastazzo_core::Id;
use pastazzo_core::account::AccountKey;
use pastazzo_core::api::B64;
use pastazzo_core::device::DeviceKeys;
use pastazzo_core::request::Scope;
use pastazzo_core::server::ServerIdentity;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::Result;

pub struct State {
    pub server_url: String,
    pub identity: ServerIdentity,
    pub username: String,
    pub account: Id,
    pub account_key: AccountKey,
    pub device: DeviceKeys,
    pub device_name: String,
    /// Where to resume reading items from.
    pub cursor: u64,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    server_url: String,
    identity: B64,
    username: String,
    account: B64,
    epoch: u32,
    account_key: B64,
    device: B64,
    device_name: String,
    cursor: u64,
}

impl Drop for Stored {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.account_key.0.zeroize();
        self.device.0.zeroize();
    }
}

impl State {
    pub fn scope(&self) -> Scope {
        Scope {
            server_fingerprint: self.identity.fingerprint(),
            account: self.account,
        }
    }

    /// `$PASTAZZO_SYNC_STATE`, or `sync.json` in the platform config dir.
    pub fn default_path() -> Result<PathBuf> {
        if let Ok(path) = std::env::var("PASTAZZO_SYNC_STATE") {
            return Ok(PathBuf::from(path));
        }
        let home = PathBuf::from(std::env::var("HOME").map_err(|_| "HOME is not set")?);
        let dir = if cfg!(target_os = "macos") {
            home.join("Library/Application Support/pastazzo")
        } else if let Ok(config) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(config).join("pastazzo")
        } else {
            home.join(".config/pastazzo")
        };
        Ok(dir.join("sync.json"))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = Zeroizing::new(fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "not logged in: run `pastazzo-sync join` or `pastazzo-sync login` first".to_owned()
            } else {
                format!("read {}: {e}", path.display())
            }
        })?);
        let stored: Stored =
            serde_json::from_str(&text).map_err(|e| format!("read {}: {e}", path.display()))?;
        let malformed = |what: &str| format!("{}: invalid {what}", path.display());
        Ok(Self {
            identity: ServerIdentity::from_bytes(&stored.identity.0)
                .map_err(|_| malformed("server identity"))?,
            account: stored.account.array().ok_or_else(|| malformed("account"))?,
            account_key: AccountKey::from_bytes(
                stored.epoch,
                stored
                    .account_key
                    .array()
                    .ok_or_else(|| malformed("account key"))?,
            )
            .map_err(|_| malformed("account key"))?,
            device: DeviceKeys::from_secret_bytes(&stored.device.0)
                .map_err(|_| malformed("device keys"))?,
            server_url: stored.server_url.clone(),
            username: stored.username.clone(),
            device_name: stored.device_name.clone(),
            cursor: stored.cursor,
        })
    }

    /// Writes the state atomically, readable only by the owner.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.save_with_cursor(path, self.cursor)
    }

    /// [`State::save`] with another cursor, for the sync loop.
    pub fn save_with_cursor(&self, path: &Path, cursor: u64) -> Result<()> {
        let stored = Stored {
            server_url: self.server_url.clone(),
            identity: B64(self.identity.to_bytes().to_vec()),
            username: self.username.clone(),
            account: B64(self.account.to_vec()),
            epoch: self.account_key.epoch(),
            account_key: B64(self.account_key.expose_secret().to_vec()),
            device: B64(self.device.to_secret_bytes().to_vec()),
            device_name: self.device_name.clone(),
            cursor,
        };
        let json = Zeroizing::new(serde_json::to_vec_pretty(&stored).map_err(|e| e.to_string())?);
        let dir = path.parent().ok_or("state path has no parent directory")?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("create {}: {e}", dir.display()))?;
        let tmp = path.with_extension("json.tmp");
        let _ = fs::remove_file(&tmp);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        file.write_all(&json).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| format!("write {}: {e}", path.display()))
    }
}
