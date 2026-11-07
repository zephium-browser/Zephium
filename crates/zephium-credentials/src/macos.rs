//! Login-Keychain generic passwords addressed by service, under one account.

use std::sync::OnceLock;

use core_foundation::bundle::CFBundle;
use core_foundation::string::CFString;
use security_framework::item::{ItemClass, ItemSearchOptions};
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::os::macos::passwords::find_generic_password;
use security_framework_sys::base::errSecItemNotFound;
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
    // Delete + add: updating in place first reads the old secret, which
    // prompts when another build of the app created the item.
    let _ = search(service, &keychain).delete();
    keychain
        .add_generic_password(service, account(), bytes)
        .or_else(|_| keychain.set_generic_password(service, account(), bytes))
        .map_err(|_| VaultError::Inaccessible)
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
    use super::account_for;

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
