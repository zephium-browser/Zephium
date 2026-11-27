//! `chrome.privacy` for the settings Zephium actually has. Password managers
//! turn the browser's own password filling off through it. Settings Zephium
//! has no counterpart for answer as not controllable, as Chrome does when a
//! policy holds them.

use std::collections::HashMap;
use std::rc::Rc;

use objc2_foundation::{NSString, NSUserDefaults};
use serde_json::{json, Map, Value};

use objc2::rc::Retained;

use crate::runtime::Shared;
use crate::{api, events};

/// Controllable settings and their value when no extension controls them.
const SETTINGS: &[(&str, bool)] = &[
    ("services.passwordSavingEnabled", true),
    ("services.autofillEnabled", true),
    ("services.autofillAddressEnabled", false),
    ("services.autofillCreditCardEnabled", false),
];

/// Who controls each setting in one profile; the latest extension to set it.
#[derive(Default)]
pub(crate) struct Overrides(std::cell::RefCell<HashMap<String, (String, bool)>>);

fn default_of(setting: &str) -> Option<bool> {
    SETTINGS
        .iter()
        .find(|(name, _)| *name == setting)
        .map(|(_, value)| *value)
}

fn details(shared: &Shared, extension: &str, setting: &str) -> Value {
    let Some(default) = default_of(setting) else {
        return json!({ "value": false, "levelOfControl": "not_controllable" });
    };
    match shared.privacy.0.borrow().get(setting) {
        Some((owner, value)) if owner == extension => {
            json!({ "value": value, "levelOfControl": "controlled_by_this_extension" })
        }
        Some((_, value)) => {
            json!({ "value": value, "levelOfControl": "controlled_by_other_extensions" })
        }
        None => json!({ "value": default, "levelOfControl": "controllable_by_this_extension" }),
    }
}

fn setting_of(message: &Value) -> Result<String, String> {
    message
        .get("setting")
        .and_then(Value::as_str)
        .filter(|setting| setting.len() <= 64)
        .map(str::to_owned)
        .ok_or_else(|| "missing setting".to_owned())
}

pub(crate) fn get(shared: &Shared, extension: &str, message: &Value) -> Result<Value, String> {
    Ok(details(shared, extension, &setting_of(message)?))
}

pub(crate) fn set(shared: &Rc<Shared>, extension: &str, message: &Value) -> Result<Value, String> {
    let setting = setting_of(message)?;
    if default_of(&setting).is_none() {
        return Err("Zephium doesn't let extensions change this setting.".into());
    }
    let value = message
        .get("value")
        .and_then(Value::as_bool)
        .ok_or("The value must be a boolean.")?;
    shared
        .privacy
        .0
        .borrow_mut()
        .insert(setting.clone(), (extension.to_owned(), value));
    remember(shared, extension, &setting, Some(value));
    changed(shared, &setting);
    Ok(Value::Null)
}

pub(crate) fn clear(
    shared: &Rc<Shared>,
    extension: &str,
    message: &Value,
) -> Result<Value, String> {
    let setting = setting_of(message)?;
    let cleared = {
        let mut overrides = shared.privacy.0.borrow_mut();
        match overrides.get(&setting) {
            Some((owner, _)) if owner == extension => overrides.remove(&setting).is_some(),
            _ => false,
        }
    };
    if cleared {
        remember(shared, extension, &setting, None);
        changed(shared, &setting);
    }
    Ok(Value::Null)
}

/// Whether the browser should offer its own password filling in this profile.
pub(crate) fn password_filling(shared: &Shared) -> bool {
    let value = |setting: &str| match shared.privacy.0.borrow().get(setting) {
        Some((_, value)) => *value,
        None => default_of(setting).unwrap_or(true),
    };
    value("services.passwordSavingEnabled") && value("services.autofillEnabled")
}

fn changed(shared: &Rc<Shared>, setting: &str) {
    shared
        .host()
        .password_filling_changed(password_filling(shared));
    let event = format!("privacy.{setting}.onChange");
    let listeners: Vec<String> = shared
        .loaded
        .borrow()
        .keys()
        .filter(|id| api::granted(shared, id, "privacy") && events::listened(shared, id, &event))
        .cloned()
        .collect();
    for id in listeners {
        let args = json!([details(shared, &id, setting)]);
        events::emit(shared, &id, &event, args, None);
    }
}

/// An unloaded extension, or one that gave up the permission, controls
/// nothing any more.
pub(crate) fn forget(shared: &Rc<Shared>, extension: &str) {
    let released: Vec<String> = {
        let mut overrides = shared.privacy.0.borrow_mut();
        let settings: Vec<String> = overrides
            .iter()
            .filter(|(_, (owner, _))| owner == extension)
            .map(|(setting, _)| setting.clone())
            .collect();
        for setting in &settings {
            overrides.remove(setting);
        }
        settings
    };
    for setting in released {
        changed(shared, &setting);
    }
}

pub(crate) fn forget_if_revoked(shared: &Rc<Shared>, extension: &str) {
    if !api::granted(shared, extension, "privacy") {
        forget(shared, extension);
        erase(shared, extension);
    }
}

// What extensions set lasts across launches, as in Chrome: password managers
// turn the browser's filling off once, at setup. It is kept per profile and
// applies again whenever the extension is loaded.

fn defaults_key(shared: &Shared) -> Retained<NSString> {
    let store = unsafe { shared.store.identifier() }
        .map(|id| id.UUIDString().to_string())
        .unwrap_or_else(|| "default".to_owned());
    NSString::from_str(&format!("ZephiumExtensionPrivacy.{store}"))
}

fn saved(shared: &Shared) -> Map<String, Value> {
    NSUserDefaults::standardUserDefaults()
        .stringForKey(&defaults_key(shared))
        .and_then(|text| serde_json::from_str(&text.to_string()).ok())
        .unwrap_or_default()
}

fn save(shared: &Shared, all: &Map<String, Value>) {
    let defaults = NSUserDefaults::standardUserDefaults();
    let key = defaults_key(shared);
    if all.is_empty() {
        defaults.removeObjectForKey(&key);
    } else {
        let text = NSString::from_str(&Value::Object(all.clone()).to_string());
        unsafe { defaults.setObject_forKey(Some(&text), &key) };
    }
}

fn remember(shared: &Shared, extension: &str, setting: &str, value: Option<bool>) {
    let mut all = saved(shared);
    let mine = all
        .entry(extension.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(mine) = mine {
        match value {
            Some(value) => {
                mine.insert(setting.to_owned(), Value::Bool(value));
            }
            None => {
                mine.remove(setting);
            }
        }
        if mine.is_empty() {
            all.remove(extension);
        }
    }
    save(shared, &all);
}

/// Drops what an uninstalled extension, or one without the permission, set.
pub(crate) fn erase(shared: &Shared, extension: &str) {
    let mut all = saved(shared);
    if all.remove(extension).is_some() {
        save(shared, &all);
    }
}

/// Applies again what a just-loaded extension set before, where no other
/// extension has taken the setting since.
pub(crate) fn restore(shared: &Rc<Shared>, extension: &str) {
    if !api::granted(shared, extension, "privacy") {
        return;
    }
    let Some(Value::Object(mine)) = saved(shared).remove(extension) else {
        return;
    };
    let applied: Vec<String> = {
        let mut overrides = shared.privacy.0.borrow_mut();
        let mut applied = Vec::new();
        for (setting, value) in mine {
            let Some(value) = value.as_bool() else {
                continue;
            };
            if default_of(&setting).is_some() && !overrides.contains_key(&setting) {
                overrides.insert(setting.clone(), (extension.to_owned(), value));
                applied.push(setting);
            }
        }
        applied
    };
    for setting in applied {
        changed(shared, &setting);
    }
}
