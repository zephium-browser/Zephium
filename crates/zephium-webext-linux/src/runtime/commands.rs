//! Effective browser-window commands, resolved after browser reservations.
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use zephium_core::accelerator::{Accelerator, Platform};
use zephium_core::ids::ExtensionInstallId;
use zephium_core::ports::engine::Shortcut;

type Chord = (u32, bool, bool, bool);

fn chord(key: &Shortcut) -> Chord {
    (key.key, key.ctrl, key.shift, key.alt)
}

fn parse(text: &str) -> Option<Chord> {
    if text.len() > 64 {
        return None;
    }
    let tokens: Vec<_> = text.split('+').collect();
    let key = *tokens.last()?;
    let ordinary = key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric();
    if !ordinary
        && !matches!(
            key,
            "Comma"
                | "Period"
                | "Home"
                | "End"
                | "PageUp"
                | "PageDown"
                | "Space"
                | "Up"
                | "Down"
                | "Left"
                | "Right"
                | "Insert"
                | "Delete"
                | "MediaNextTrack"
                | "MediaPrevTrack"
                | "MediaStop"
                | "MediaPlayPause"
        )
    {
        return None;
    }
    let media = key.starts_with("Media");
    if media {
        return (tokens.len() == 1).then_some((
            match key {
                "MediaNextTrack" => 0xB0,
                "MediaPrevTrack" => 0xB1,
                "MediaStop" => 0xB2,
                _ => 0xB3,
            },
            false,
            false,
            false,
        ));
    }
    if tokens[..tokens.len() - 1]
        .iter()
        .any(|token| !matches!(*token, "Ctrl" | "Alt" | "Shift"))
    {
        return None;
    }
    let parsed = Accelerator::parse(&text.replace("Insert", "Delete"), Platform::Other)?;
    if parsed.ctrl == parsed.alt {
        return None;
    }
    Some((
        if key == "Insert" {
            0x2D
        } else {
            parsed.key.virtual_key()
        },
        parsed.ctrl,
        parsed.shift,
        parsed.alt,
    ))
}

#[derive(Default)]
pub(crate) struct Commands {
    manifests: BTreeMap<ExtensionInstallId, Value>,
    reserved: Vec<Shortcut>,
    effective: BTreeMap<ExtensionInstallId, Vec<Value>>,
    bindings: BTreeMap<Chord, (ExtensionInstallId, String)>,
}

impl Commands {
    pub(crate) fn install(&mut self, install: ExtensionInstallId, manifest: &Value) {
        self.manifests.insert(
            install,
            manifest.get("commands").cloned().unwrap_or(Value::Null),
        );
        self.resolve();
    }
    pub(crate) fn remove(&mut self, install: ExtensionInstallId) {
        self.manifests.remove(&install);
        self.resolve();
    }
    pub(crate) fn reserve(&mut self, reserved: Vec<Shortcut>) {
        self.reserved = reserved;
        self.resolve();
    }
    pub(crate) fn inventory(&self, install: ExtensionInstallId) -> Vec<Value> {
        self.effective.get(&install).cloned().unwrap_or_default()
    }
    pub(crate) fn find(
        &self,
        key: u32,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> Option<(ExtensionInstallId, String)> {
        self.bindings.get(&(key, ctrl, shift, alt)).cloned()
    }
    fn resolve(&mut self) {
        self.effective.clear();
        self.bindings.clear();
        let mut used: HashSet<Chord> = self.reserved.iter().map(chord).collect();
        for (install, commands) in &self.manifests {
            let Some(commands) = commands.as_object() else {
                continue;
            };
            let mut inventory = Vec::new();
            for (name, command) in commands.iter().take(100) {
                let suggested = &command["suggested_key"];
                let suggested = suggested
                    .as_str()
                    .or_else(|| suggested["linux"].as_str())
                    .or_else(|| suggested["default"].as_str())
                    .unwrap_or("");
                let binding = parse(suggested).filter(|key| used.insert(*key));
                if let Some(binding) = binding {
                    self.bindings.insert(binding, (*install, name.clone()));
                }
                inventory.push(json!({"name":name,"description":command["description"].as_str().unwrap_or(""),"shortcut":if binding.is_some() { suggested } else { "" }}));
            }
            self.effective.insert(*install, inventory);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_reservations_and_install_order_control_effective_shortcuts() {
        let mut commands = Commands::default();
        let a = ExtensionInstallId::from(1);
        let b = ExtensionInstallId::from(2);
        let manifest = json!({"commands":{"go":{"description":"Go","suggested_key":{"default":"Ctrl+Y","linux":"Alt+Shift+Y"}}}});
        commands.install(b, &manifest);
        commands.install(a, &manifest);
        assert_eq!(commands.inventory(a)[0]["shortcut"], "Alt+Shift+Y");
        assert_eq!(commands.inventory(b)[0]["shortcut"], "");
        commands.reserve(vec![Shortcut {
            id: "browser".into(),
            key: 89,
            ctrl: false,
            shift: true,
            alt: true,
        }]);
        assert_eq!(commands.inventory(a)[0]["shortcut"], "");
        assert!(commands.find(89, false, true, true).is_none());
        commands.reserve(Vec::new());
        commands.remove(a);
        assert_eq!(commands.find(89, false, true, true), Some((b, "go".into())));
        for key in [
            "Y",
            "Shift+Y",
            "Ctrl+Alt+Y",
            "Ctrl+Ctrl+Y",
            "Super+Y",
            "Ctrl+Escape",
            "Ctrl++Y",
        ] {
            assert!(parse(key).is_none(), "{key}");
        }
        assert_eq!(parse("Ctrl+Insert"), Some((0x2D, true, false, false)));
    }
}
