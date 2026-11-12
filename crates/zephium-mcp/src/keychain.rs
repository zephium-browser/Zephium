//! Connection secrets as login-Keychain generic passwords: one service per
//! profile and server, one account per secret (`bearer`, `oauth`,
//! `env.NAME`). Presence is answered from attributes, never by reading.
use std::sync::Arc;

use crate::oauth::TokenStore;

/// Content-free Keychain failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeychainError {
    Missing,
    Inaccessible,
}

/// Injectable boundary for connection secrets; tests provide an in-memory vault.
pub trait SecretVault {
    fn read(&self, profile: &str, server: &str, account: &str) -> Result<String, KeychainError>;
    fn write(
        &self,
        profile: &str,
        server: &str,
        account: &str,
        secret: &str,
    ) -> Result<(), KeychainError>;
    fn delete(&self, profile: &str, server: &str, account: &str) -> Result<(), KeychainError>;
    fn delete_server(&self, profile: &str, server: &str) -> Result<(), KeychainError>;
}

pub struct SystemVault;
impl SecretVault for SystemVault {
    fn read(&self, profile: &str, server: &str, account: &str) -> Result<String, KeychainError> {
        read(profile, server, account)
    }
    fn write(
        &self,
        profile: &str,
        server: &str,
        account: &str,
        secret: &str,
    ) -> Result<(), KeychainError> {
        write(profile, server, account, secret)
    }
    fn delete(&self, profile: &str, server: &str, account: &str) -> Result<(), KeychainError> {
        delete(profile, server, account)
    }
    fn delete_server(&self, profile: &str, server: &str) -> Result<(), KeychainError> {
        delete_server(profile, server)
    }
}

fn service(profile: &str, server: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        let encode =
            |value: &str| -> String { value.bytes().map(|byte| format!("{byte:02x}")).collect() };
        format!(
            "app.zephium.connection.{}.{}",
            encode(profile),
            encode(server)
        )
    }
    #[cfg(not(target_os = "windows"))]
    format!("app.zephium.connection.{profile}.{server}")
}

#[cfg(all(test, target_os = "windows"))]
mod windows_targets {
    use super::service;

    #[test]
    fn profile_server_identity_is_unambiguous_and_case_preserving() {
        assert_ne!(service("a.b", "c"), service("a", "b.c"));
        assert_ne!(service("Profile", "server"), service("profile", "server"));
        assert_ne!(service("profile", "Server"), service("profile", "server"));
    }
}

/// Reads one secret. Blocking.
pub fn read(profile: &str, server: &str, account: &str) -> Result<String, KeychainError> {
    platform::read(&service(profile, server), account)
}
/// Stores one secret, replacing an earlier one. Blocking.
pub fn write(
    profile: &str,
    server: &str,
    account: &str,
    secret: &str,
) -> Result<(), KeychainError> {
    platform::write(&service(profile, server), account, secret)
}
/// Removes one secret; removing a missing one succeeds. Blocking.
pub fn delete(profile: &str, server: &str, account: &str) -> Result<(), KeychainError> {
    platform::delete(&service(profile, server), account)
}
/// Removes every secret for a server, including accounts from earlier configurations.
pub fn delete_server(profile: &str, server: &str) -> Result<(), KeychainError> {
    platform::delete_server(&service(profile, server))
}
/// Whether a secret exists, from its attributes only. Blocking.
pub fn present(profile: &str, server: &str, account: &str) -> bool {
    platform::present(&service(profile, server), account)
}

/// A server's OAuth credentials in the Keychain.
pub fn oauth_store(profile: &str, server: &str) -> Arc<dyn TokenStore> {
    Arc::new(OAuthItem {
        profile: profile.to_owned(),
        server: server.to_owned(),
    })
}

struct OAuthItem {
    profile: String,
    server: String,
}
impl TokenStore for OAuthItem {
    fn load(&self) -> Option<String> {
        read(&self.profile, &self.server, "oauth").ok()
    }
    fn save(&self, value: &str) -> bool {
        write(&self.profile, &self.server, "oauth", value).is_ok()
    }
    fn clear(&self) {
        let _ = delete(&self.profile, &self.server, "oauth");
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use core_foundation::data::CFData;
    use security_framework::item::{
        update_item, ItemClass, ItemSearchOptions, ItemUpdateOptions, ItemUpdateValue,
    };
    use security_framework::os::macos::keychain::SecKeychain;
    use security_framework::os::macos::passwords::find_generic_password;
    use security_framework_sys::base::{errSecDuplicateItem, errSecItemNotFound};

    use super::KeychainError;

    fn login() -> Result<SecKeychain, KeychainError> {
        let home = std::env::var_os("HOME").ok_or(KeychainError::Inaccessible)?;
        SecKeychain::open(std::path::Path::new(&home).join("Library/Keychains/login.keychain-db"))
            .map_err(|_| KeychainError::Inaccessible)
    }

    fn search(service: &str, account: &str, keychain: &SecKeychain) -> ItemSearchOptions {
        let mut options = ItemSearchOptions::new();
        options
            .class(ItemClass::generic_password())
            .keychains(std::slice::from_ref(keychain))
            .service(service)
            .account(account);
        options
    }

    pub(super) fn read(service: &str, account: &str) -> Result<String, KeychainError> {
        let keychain = login()?;
        let (password, _) =
            find_generic_password(Some(&[keychain]), service, account).map_err(|error| {
                if error.code() == errSecItemNotFound {
                    KeychainError::Missing
                } else {
                    KeychainError::Inaccessible
                }
            })?;
        String::from_utf8(password.as_ref().to_vec()).map_err(|_| KeychainError::Inaccessible)
    }

    pub(super) fn write(service: &str, account: &str, secret: &str) -> Result<(), KeychainError> {
        let keychain = login()?;
        // An attribute-only update preserves the existing item on failure and
        // does not read its secret before writing the replacement.
        let mut replacement = ItemUpdateOptions::new();
        replacement.set_value(ItemUpdateValue::Data(CFData::from_buffer(
            secret.as_bytes(),
        )));
        replace_with(
            || {
                update_item(&search(service, account, &keychain), &replacement)
                    .map_err(|error| error.code())
            },
            || {
                keychain
                    .add_generic_password(service, account, secret.as_bytes())
                    .map_err(|error| error.code())
            },
        )
        .map_err(|_| KeychainError::Inaccessible)
    }

    fn replace_with(
        mut update: impl FnMut() -> Result<(), i32>,
        mut add: impl FnMut() -> Result<(), i32>,
    ) -> Result<(), i32> {
        match update() {
            Err(code) if code == errSecItemNotFound => match add() {
                Err(code) if code == errSecDuplicateItem => update(),
                result => result,
            },
            result => result,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::replace_with;

        struct MockItem {
            value: std::cell::RefCell<Option<Vec<u8>>>,
            fail_update: std::cell::Cell<bool>,
            updates: std::cell::Cell<usize>,
            additions: std::cell::Cell<usize>,
        }
        impl MockItem {
            fn update(&self, replacement: &[u8]) -> Result<(), i32> {
                self.updates.set(self.updates.get() + 1);
                if self.value.borrow().is_none() {
                    return Err(super::errSecItemNotFound);
                }
                if self.fail_update.get() {
                    return Err(-1);
                }
                self.value.replace(Some(replacement.to_vec()));
                Ok(())
            }
            fn add(&self, replacement: &[u8]) -> Result<(), i32> {
                self.additions.set(self.additions.get() + 1);
                if self.value.borrow().is_some() {
                    return Err(super::errSecDuplicateItem);
                }
                self.value.replace(Some(replacement.to_vec()));
                Ok(())
            }
        }

        #[test]
        fn failed_replacement_preserves_the_previous_secret_without_retrying_add() {
            let item = MockItem {
                value: std::cell::RefCell::new(Some(b"original".to_vec())),
                fail_update: std::cell::Cell::new(true),
                updates: std::cell::Cell::new(0),
                additions: std::cell::Cell::new(0),
            };
            assert_eq!(
                replace_with(|| item.update(b"replacement"), || item.add(b"replacement")),
                Err(-1)
            );
            assert_eq!(item.updates.get(), 1);
            assert_eq!(item.additions.get(), 0);
            assert_eq!(item.value.borrow().as_deref(), Some(b"original".as_slice()));
            item.fail_update.set(false);
            assert_eq!(
                replace_with(|| item.update(b"replacement"), || item.add(b"replacement")),
                Ok(())
            );
            assert_eq!(
                item.value.borrow().as_deref(),
                Some(b"replacement".as_slice())
            );
            assert_eq!(item.updates.get(), 2);
            assert_eq!(item.additions.get(), 0);
        }

        #[test]
        fn missing_items_are_added_and_creation_races_retry_an_atomic_update() {
            use super::{errSecDuplicateItem, errSecItemNotFound};
            let updates = std::cell::Cell::new(0);
            assert_eq!(replace_with(|| Err(errSecItemNotFound), || Ok(())), Ok(()));
            assert_eq!(
                replace_with(|| Err(errSecItemNotFound), || Err(-1)),
                Err(-1)
            );
            assert_eq!(
                replace_with(
                    || {
                        updates.set(updates.get() + 1);
                        if updates.get() == 1 {
                            Err(errSecItemNotFound)
                        } else {
                            Ok(())
                        }
                    },
                    || Err(errSecDuplicateItem)
                ),
                Ok(())
            );
            assert_eq!(updates.get(), 2);
        }
    }

    pub(super) fn delete(service: &str, account: &str) -> Result<(), KeychainError> {
        let keychain = login()?;
        match search(service, account, &keychain).delete() {
            Ok(()) => Ok(()),
            Err(error) if error.code() == errSecItemNotFound => Ok(()),
            Err(_) => Err(KeychainError::Inaccessible),
        }
    }

    pub(super) fn delete_server(service: &str) -> Result<(), KeychainError> {
        let keychain = login()?;
        let mut options = ItemSearchOptions::new();
        options
            .class(ItemClass::generic_password())
            .keychains(&[keychain])
            .service(service);
        match options.delete() {
            Ok(()) => Ok(()),
            Err(error) if error.code() == errSecItemNotFound => Ok(()),
            Err(_) => Err(KeychainError::Inaccessible),
        }
    }

    pub(super) fn present(service: &str, account: &str) -> bool {
        let Ok(keychain) = login() else {
            return false;
        };
        let mut options = search(service, account, &keychain);
        options.load_attributes(true);
        options.search().is_ok_and(|items| !items.is_empty())
    }
}

#[cfg(target_os = "windows")]
#[path = "keychain_windows.rs"]
mod platform;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::KeychainError;
    pub(super) fn read(_: &str, _: &str) -> Result<String, KeychainError> {
        Err(KeychainError::Missing)
    }
    pub(super) fn write(_: &str, _: &str, _: &str) -> Result<(), KeychainError> {
        Err(KeychainError::Inaccessible)
    }
    pub(super) fn delete(_: &str, _: &str) -> Result<(), KeychainError> {
        Ok(())
    }
    pub(super) fn delete_server(_: &str) -> Result<(), KeychainError> {
        Ok(())
    }
    pub(super) fn present(_: &str, _: &str) -> bool {
        false
    }
}
