#[cfg(not(target_os = "ios"))]
pub use pastazzo_sync::secrets::SystemKeychain as MobileKeychain;

#[cfg(target_os = "ios")]
mod ios {
    use apple_native_keyring_store::protected::Store;
    use keyring_core::api::CredentialStoreApi;
    use pastazzo_sync::Result;
    use pastazzo_sync::secrets::{KEYCHAIN_SERVICE, SecretBackend};
    use std::collections::HashMap;
    use zeroize::Zeroizing;

    pub struct MobileKeychain;

    fn entry(user: &str) -> Result<keyring_core::Entry> {
        let store = Store::new().map_err(|e| e.to_string())?;
        store
            .build(
                KEYCHAIN_SERVICE,
                user,
                Some(&HashMap::from([(
                    "access-policy",
                    "when-unlocked-this-device-only",
                )])),
            )
            .map_err(|e| e.to_string())
    }

    impl SecretBackend for MobileKeychain {
        fn available(&self) -> bool {
            true
        }
        fn set(&self, user: &str, secret: &[u8]) -> Result<()> {
            entry(user)?.set_secret(secret).map_err(|e| e.to_string())
        }
        fn get(&self, user: &str) -> Result<Zeroizing<Vec<u8>>> {
            entry(user)?
                .get_secret()
                .map(Zeroizing::new)
                .map_err(|e| e.to_string())
        }
        fn delete(&self, user: &str) -> Result<()> {
            match entry(user)?.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
    }
}

#[cfg(target_os = "ios")]
pub use ios::MobileKeychain;
