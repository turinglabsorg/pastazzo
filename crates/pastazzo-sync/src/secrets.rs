//! Where a device keeps its secrets, the account key and its own keys: the
//! system keychain whenever there is one.

use std::collections::HashMap;
use std::sync::Mutex;

use zeroize::Zeroizing;

use crate::Result;

/// The keychain entries' service name; the user name identifies the device.
pub const KEYCHAIN_SERVICE: &str = "pastazzo-sync";

pub trait SecretBackend {
    /// Whether this system has a keychain at all. A locked keychain is
    /// available: reading from it fails until it's unlocked.
    fn available(&self) -> bool;
    fn set(&self, user: &str, secret: &[u8]) -> Result<()>;
    fn get(&self, user: &str) -> Result<Zeroizing<Vec<u8>>>;
    /// Succeeds if the entry doesn't exist.
    fn delete(&self, user: &str) -> Result<()>;
}

/// The system keychain: the Secret Service (GNOME Keyring, KWallet) on
/// Linux, Keychain Services on macOS. `PASTAZZO_KEY_STORAGE=file` turns it
/// off, for systems where it's there but unusable.
pub struct SystemKeychain;

fn describe(error: keyring::Error) -> String {
    match error {
        keyring::Error::NoDefaultStore => "this system has no keychain".to_owned(),
        keyring::Error::NoStorageAccess(error) => {
            format!("the keychain is locked or refused access ({error})")
        }
        keyring::Error::NoEntry => "the keys aren't in the keychain".to_owned(),
        other => format!("keychain: {other}"),
    }
}

impl SystemKeychain {
    fn entry(user: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, user).map_err(describe)
    }
}

impl SecretBackend for SystemKeychain {
    fn available(&self) -> bool {
        if std::env::var("PASTAZZO_KEY_STORAGE").as_deref() == Ok("file") {
            return false;
        }
        !matches!(
            keyring::Entry::new(KEYCHAIN_SERVICE, "availability"),
            Err(keyring::Error::NoDefaultStore)
        )
    }

    fn set(&self, user: &str, secret: &[u8]) -> Result<()> {
        Self::entry(user)?.set_secret(secret).map_err(describe)
    }

    fn get(&self, user: &str) -> Result<Zeroizing<Vec<u8>>> {
        Self::entry(user)?
            .get_secret()
            .map(Zeroizing::new)
            .map_err(describe)
    }

    fn delete(&self, user: &str) -> Result<()> {
        match Self::entry(user)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(describe(error)),
        }
    }
}

/// No keychain: the secrets stay in the state file.
pub struct NoKeychain;

impl SecretBackend for NoKeychain {
    fn available(&self) -> bool {
        false
    }

    fn set(&self, _: &str, _: &[u8]) -> Result<()> {
        Err("no keychain".to_owned())
    }

    fn get(&self, _: &str) -> Result<Zeroizing<Vec<u8>>> {
        Err("no keychain".to_owned())
    }

    fn delete(&self, _: &str) -> Result<()> {
        Ok(())
    }
}

/// A keychain in memory, for tests. `locked` makes every access fail, the
/// way a locked keychain does.
#[derive(Default)]
pub struct MemoryKeychain {
    pub entries: Mutex<HashMap<String, Vec<u8>>>,
    pub locked: std::sync::atomic::AtomicBool,
}

impl MemoryKeychain {
    fn check(&self) -> Result<()> {
        if self.locked.load(std::sync::atomic::Ordering::SeqCst) {
            Err("the keychain is locked".to_owned())
        } else {
            Ok(())
        }
    }
}

impl SecretBackend for MemoryKeychain {
    fn available(&self) -> bool {
        true
    }

    fn set(&self, user: &str, secret: &[u8]) -> Result<()> {
        self.check()?;
        self.entries
            .lock()
            .unwrap()
            .insert(user.to_owned(), secret.to_vec());
        Ok(())
    }

    fn get(&self, user: &str) -> Result<Zeroizing<Vec<u8>>> {
        self.check()?;
        self.entries
            .lock()
            .unwrap()
            .get(user)
            .cloned()
            .map(Zeroizing::new)
            .ok_or_else(|| "the keys aren't in the keychain".to_owned())
    }

    fn delete(&self, user: &str) -> Result<()> {
        self.check()?;
        self.entries.lock().unwrap().remove(user);
        Ok(())
    }
}
