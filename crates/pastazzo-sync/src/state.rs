//! What a logged-in device keeps: the server, the account key and its own
//! keys.
//!
//! The account key and the device keys go in the system keychain (see
//! [`crate::secrets`]); the state file, readable only by the owner, keeps
//! the rest. Only on a system without any keychain do the keys stay in the
//! file. Keys found in a file from an older version move to the keychain the
//! first time the file is read.

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

use crate::secrets::{SecretBackend, SystemKeychain};
use crate::{Result, log};

/// Where this device's keys are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyStorage {
    Keychain,
    File,
}

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
    pub storage: KeyStorage,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    server_url: String,
    identity: B64,
    username: String,
    account: B64,
    device_name: String,
    cursor: u64,
    /// Missing in files from before the keychain: those have the keys inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_storage: Option<KeyStorage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_id: Option<B64>,
    // Only when the keys are in the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    epoch: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_key: Option<B64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device: Option<B64>,
}

impl Drop for Stored {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(key) = &mut self.account_key {
            key.0.zeroize();
        }
        if let Some(device) = &mut self.device {
            device.0.zeroize();
        }
    }
}

/// `version(1) || u32be(epoch) || account key(32) || device secret keys`
const SECRETS_VERSION: u8 = 1;

fn host(server_url: &str) -> &str {
    let rest = server_url
        .split_once("://")
        .map_or(server_url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest)
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

    /// The keychain entry's user name: one entry per device and account.
    fn keychain_user(username: &str, server_url: &str, device: &Id) -> String {
        format!("{username}@{} {}", host(server_url), B64::encode(device))
    }

    fn secrets(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(vec![SECRETS_VERSION]);
        out.extend_from_slice(&self.account_key.epoch().to_be_bytes());
        out.extend_from_slice(self.account_key.expose_secret());
        out.extend_from_slice(&self.device.to_secret_bytes());
        out
    }

    fn parse_secrets(bytes: &[u8]) -> Result<(AccountKey, DeviceKeys)> {
        let malformed = || "the keys in the keychain are malformed".to_owned();
        if bytes.len() < 37 || bytes[0] != SECRETS_VERSION {
            return Err(malformed());
        }
        let epoch = u32::from_be_bytes(bytes[1..5].try_into().expect("4 bytes"));
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&bytes[5..37]);
        let account_key = AccountKey::from_bytes(epoch, *key).map_err(|_| malformed())?;
        let device = DeviceKeys::from_secret_bytes(&bytes[37..]).map_err(|_| malformed())?;
        Ok((account_key, device))
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with(path, &SystemKeychain)
    }

    pub fn load_with(path: &Path, keychain: &dyn SecretBackend) -> Result<Self> {
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

        let (account_key, device, storage) = if stored.key_storage == Some(KeyStorage::Keychain) {
            let device_id: Id = stored
                .device_id
                .as_ref()
                .and_then(B64::array)
                .ok_or_else(|| malformed("device id"))?;
            let user = Self::keychain_user(&stored.username, &stored.server_url, &device_id);
            let secrets = keychain
                .get(&user)
                .map_err(|e| format!("can't read this device's keys: {e}"))?;
            let (account_key, device) = Self::parse_secrets(&secrets)?;
            if device.id() != device_id {
                return Err("the keys in the keychain belong to another device".to_owned());
            }
            (account_key, device, KeyStorage::Keychain)
        } else {
            let key = stored
                .account_key
                .as_ref()
                .and_then(B64::array)
                .ok_or_else(|| malformed("account key"))?;
            let account_key = AccountKey::from_bytes(stored.epoch.unwrap_or(0), key)
                .map_err(|_| malformed("account key"))?;
            let device_secret = stored
                .device
                .as_ref()
                .ok_or_else(|| malformed("device keys"))?;
            let device = DeviceKeys::from_secret_bytes(&device_secret.0)
                .map_err(|_| malformed("device keys"))?;
            (account_key, device, KeyStorage::File)
        };

        let mut state = Self {
            identity: ServerIdentity::from_bytes(&stored.identity.0)
                .map_err(|_| malformed("server identity"))?,
            account: stored.account.array().ok_or_else(|| malformed("account"))?,
            account_key,
            device,
            server_url: stored.server_url.clone(),
            username: stored.username.clone(),
            device_name: stored.device_name.clone(),
            cursor: stored.cursor,
            storage,
        };
        if state.storage == KeyStorage::File && keychain.available() {
            match state.move_to_keychain(path, keychain) {
                Ok(()) => log!("moved this device's keys into the system keychain"),
                Err(error) => log!(
                    "this device's keys stay in {} for now: {error}",
                    path.display()
                ),
            }
        }
        Ok(state)
    }

    /// Saves a device that just logged in: its keys go in the keychain if
    /// there is one, otherwise in the file.
    pub fn save_new(&mut self, path: &Path) -> Result<()> {
        self.save_new_with(path, &SystemKeychain)
    }

    pub fn save_new_with(&mut self, path: &Path, keychain: &dyn SecretBackend) -> Result<()> {
        if keychain.available() {
            keychain
                .set(&self.user(), &self.secrets())
                .map_err(|e| format!("can't save the keys: {e}"))?;
            self.storage = KeyStorage::Keychain;
        } else {
            log!(
                "no keychain on this system: the keys are kept in {}",
                path.display()
            );
            self.storage = KeyStorage::File;
        }
        self.save(path)
    }

    fn user(&self) -> String {
        Self::keychain_user(&self.username, &self.server_url, &self.device.id())
    }

    fn move_to_keychain(&mut self, path: &Path, keychain: &dyn SecretBackend) -> Result<()> {
        let secrets = self.secrets();
        keychain.set(&self.user(), &secrets)?;
        // Only drop the file's copy once the keychain has it.
        if *keychain.get(&self.user())? != *secrets {
            return Err("the keychain didn't keep the keys".to_owned());
        }
        self.storage = KeyStorage::Keychain;
        scrub(path);
        self.save(path)
    }

    /// Removes this device's keys and state, on logout.
    pub fn forget(&self, path: &Path) -> Result<()> {
        self.forget_with(path, &SystemKeychain)
    }

    pub fn forget_with(&self, path: &Path, keychain: &dyn SecretBackend) -> Result<()> {
        if self.storage == KeyStorage::Keychain {
            keychain.delete(&self.user())?;
        } else {
            scrub(path);
        }
        fs::remove_file(path).map_err(|e| format!("remove {}: {e}", path.display()))
    }

    /// Writes the state atomically, readable only by the owner.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.save_with_cursor(path, self.cursor)
    }

    /// [`State::save`] with another cursor, for the sync loop.
    pub fn save_with_cursor(&self, path: &Path, cursor: u64) -> Result<()> {
        let in_file = self.storage == KeyStorage::File;
        let stored = Stored {
            server_url: self.server_url.clone(),
            identity: B64(self.identity.to_bytes().to_vec()),
            username: self.username.clone(),
            account: B64(self.account.to_vec()),
            device_name: self.device_name.clone(),
            cursor,
            key_storage: Some(self.storage),
            device_id: Some(B64(self.device.id().to_vec())),
            epoch: in_file.then(|| self.account_key.epoch()),
            account_key: in_file.then(|| B64(self.account_key.expose_secret().to_vec())),
            device: in_file.then(|| B64(self.device.to_secret_bytes().to_vec())),
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

/// Overwrites a file that held keys before it's replaced or removed. Best
/// effort: the file system or the disk may still keep older copies.
fn scrub(path: &Path) {
    if let Ok(len) = fs::metadata(path).map(|m| m.len())
        && let Ok(mut file) = OpenOptions::new().write(true).open(path)
    {
        let _ = file.write_all(&vec![0u8; len as usize]);
        let _ = file.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{MemoryKeychain, NoKeychain};
    use pastazzo_core::server::ServerKeys;
    use rand::rngs::OsRng;

    fn state() -> State {
        State {
            server_url: "https://clip.example.org".into(),
            identity: ServerKeys::generate(&mut OsRng).identity(),
            username: "seb".into(),
            account: [1; 16],
            account_key: AccountKey::generate(&mut OsRng),
            device: DeviceKeys::generate(&mut OsRng),
            device_name: "XPS".into(),
            cursor: 7,
            storage: KeyStorage::File,
        }
    }

    fn temp_path() -> PathBuf {
        std::env::temp_dir()
            .join(format!("pastazzo-state-{}", rand::random::<u64>()))
            .join("sync.json")
    }

    fn file_has_keys(path: &Path, state: &State) -> bool {
        let text = fs::read_to_string(path).unwrap();
        text.contains(&B64::encode(state.account_key.expose_secret()))
            || text.contains("\"account_key\"")
    }

    #[test]
    fn new_devices_keep_their_keys_in_the_keychain() {
        let keychain = MemoryKeychain::default();
        let path = temp_path();
        let mut state = state();
        state.save_new_with(&path, &keychain).unwrap();
        assert_eq!(state.storage, KeyStorage::Keychain);
        assert!(!file_has_keys(&path, &state));

        let loaded = State::load_with(&path, &keychain).unwrap();
        assert_eq!(
            loaded.account_key.expose_secret(),
            state.account_key.expose_secret()
        );
        assert_eq!(loaded.device.public(), state.device.public());
        assert_eq!(loaded.cursor, 7);

        // The cursor moving on doesn't put the keys back in the file.
        loaded.save_with_cursor(&path, 9).unwrap();
        assert!(!file_has_keys(&path, &state));
    }

    #[test]
    fn keys_in_an_old_file_move_to_the_keychain() {
        let path = temp_path();
        let state = state();
        state.save(&path).unwrap();
        assert!(file_has_keys(&path, &state));

        let keychain = MemoryKeychain::default();
        let migrated = State::load_with(&path, &keychain).unwrap();
        assert_eq!(migrated.storage, KeyStorage::Keychain);
        assert!(!file_has_keys(&path, &state));
        assert_eq!(keychain.entries.lock().unwrap().len(), 1);
        let reloaded = State::load_with(&path, &keychain).unwrap();
        assert_eq!(
            reloaded.account_key.expose_secret(),
            state.account_key.expose_secret()
        );
    }

    #[test]
    fn a_locked_keychain_is_an_error_not_a_fallback() {
        let keychain = MemoryKeychain::default();
        let path = temp_path();
        state().save_new_with(&path, &keychain).unwrap();
        keychain
            .locked
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let error = State::load_with(&path, &keychain).err().unwrap();
        assert!(error.contains("locked"), "{error}");
    }

    #[test]
    fn a_failed_migration_keeps_the_keys_in_the_file() {
        let path = temp_path();
        let state = state();
        state.save(&path).unwrap();
        let keychain = MemoryKeychain::default();
        keychain
            .locked
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let loaded = State::load_with(&path, &keychain).unwrap();
        assert_eq!(loaded.storage, KeyStorage::File);
        assert!(file_has_keys(&path, &state));
    }

    #[test]
    fn without_a_keychain_keys_stay_in_the_file() {
        let path = temp_path();
        let mut state = state();
        state.save_new_with(&path, &NoKeychain).unwrap();
        assert_eq!(state.storage, KeyStorage::File);
        assert!(State::load_with(&path, &NoKeychain).is_ok());
    }

    #[test]
    fn forgetting_removes_the_keys_too() {
        let keychain = MemoryKeychain::default();
        let path = temp_path();
        let mut state = state();
        state.save_new_with(&path, &keychain).unwrap();
        state.forget_with(&path, &keychain).unwrap();
        assert!(!path.exists());
        assert!(keychain.entries.lock().unwrap().is_empty());
    }

    #[test]
    fn host_of_the_server_url() {
        assert_eq!(
            host("https://pastazzo.grooooog.space"),
            "pastazzo.grooooog.space"
        );
        assert_eq!(host("http://127.0.0.1:4320/"), "127.0.0.1:4320");
    }
}
