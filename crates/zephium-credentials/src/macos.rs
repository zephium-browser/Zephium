//! Login-Keychain generic passwords addressed by service, under one account.

use std::sync::OnceLock;

use core_foundation::bundle::CFBundle;
use core_foundation::data::CFData;
use core_foundation::string::CFString;
use security_framework::item::{
    update_item, ItemClass, ItemSearchOptions, ItemUpdateOptions, ItemUpdateValue,
};
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::os::macos::passwords::find_generic_password;
use security_framework_sys::base::{errSecDuplicateItem, errSecItemNotFound};
use zeroize::Zeroizing;

use crate::VaultError;

const SHARED: &str = "zephium";
const MAX_SECRET_BYTES: usize = 4096;

/// Builds under another identity (QA, probes, benchmarks) keep items of their
/// own: reading an item the shipped app made, from a build signed as something
/// else, makes macOS ask the person for their login password.
fn account() -> &'static str {
    static ACCOUNT: OnceLock<String> = OnceLock::new();
    ACCOUNT.get_or_init(|| account_for(bundle_identifier().as_deref()))
}

fn account_for(identifier: Option<&str>) -> String {
    match identifier {
        Some(identifier) if identifier != "app.zephium" && identifier != "app.zephium.dev" => {
            identifier.to_owned()
        }
        _ => SHARED.to_owned(),
    }
}

fn bundle_identifier() -> Option<String> {
    let info = CFBundle::main_bundle().info_dictionary();
    let value = info.find(CFString::from_static_string("CFBundleIdentifier"))?;
    value
        .downcast::<CFString>()
        .map(|identifier| identifier.to_string())
}
fn login() -> Result<SecKeychain, VaultError> {
    let path = dirs::home_dir()
        .ok_or(VaultError::Inaccessible)?
        .join("Library/Keychains/login.keychain-db");
    SecKeychain::open(path).map_err(|_| VaultError::Inaccessible)
}

fn search(service: &str, keychain: &SecKeychain) -> ItemSearchOptions {
    let mut options = ItemSearchOptions::new();
    options
        .class(ItemClass::generic_password())
        .keychains(std::slice::from_ref(keychain))
        .service(service)
        .account(account());
    options
}

fn not_found(code: i32) -> VaultError {
    if code == errSecItemNotFound {
        VaultError::Missing
    } else {
        VaultError::Inaccessible
    }
}

/// Copies one bounded secret into zeroizing storage.
pub fn read(service: &str) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let keychain = login()?;
    let (password, _item) = find_generic_password(Some(&[keychain]), service, account())
        .map_err(|error| not_found(error.code()))?;
    if password.len() > MAX_SECRET_BYTES {
        return Err(VaultError::Invalid);
    }
    Ok(Zeroizing::new(password.as_ref().to_vec()))
}

/// Tests presence from attributes alone, never reading the secret.
pub fn present(service: &str) -> Result<bool, VaultError> {
    let keychain = login()?;
    let mut options = search(service, &keychain);
    options.load_attributes(true);
    match options.search() {
        Ok(items) => Ok(!items.is_empty()),
        Err(error) if error.code() == errSecItemNotFound => Ok(false),
        Err(_) => Err(VaultError::Inaccessible),
    }
}

/// Replaces the item.
pub fn write(service: &str, bytes: &[u8]) -> Result<(), VaultError> {
    if bytes.is_empty() || bytes.len() > MAX_SECRET_BYTES {
        return Err(VaultError::Invalid);
    }
    let keychain = login()?;
    // Update by attributes: never read or remove the old secret first.
    let mut replacement = ItemUpdateOptions::new();
    replacement.set_value(ItemUpdateValue::Data(CFData::from_buffer(bytes)));
    replace_with(
        || update_item(&search(service, &keychain), &replacement).map_err(|error| error.code()),
        || {
            keychain
                .add_generic_password(service, account(), bytes)
                .map_err(|error| error.code())
        },
    )
    .map_err(|_| VaultError::Inaccessible)
}

fn replace_with(
    mut update: impl FnMut() -> Result<(), i32>,
    mut add: impl FnMut() -> Result<(), i32>,
) -> Result<(), i32> {
    match update() {
        Err(code) if code == errSecItemNotFound => match add() {
            // Another writer may have created it after the absence check.
            Err(code) if code == errSecDuplicateItem => update(),
            result => result,
        },
        result => result,
    }
}

/// Deletes the item; deleting a missing item succeeds.
pub fn delete(service: &str) -> Result<(), VaultError> {
    let keychain = login()?;
    match search(service, &keychain).delete() {
        Ok(()) => Ok(()),
        Err(error) if error.code() == errSecItemNotFound => Ok(()),
        Err(_) => Err(VaultError::Inaccessible),
    }
}

#[cfg(test)]
mod tests {
    use super::{account_for, replace_with};

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

    #[test]
    fn only_other_identities_get_items_of_their_own() {
        assert_eq!(account_for(Some("app.zephium")), "zephium");
        assert_eq!(account_for(Some("app.zephium.dev")), "zephium");
        assert_eq!(account_for(None), "zephium");
        assert_eq!(
            account_for(Some("app.zephium.webext-qa")),
            "app.zephium.webext-qa"
        );
    }
}
