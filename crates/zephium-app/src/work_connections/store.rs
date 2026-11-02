//! The MCP servers a profile added, as one small JSON file per profile in the
//! app's data folder. Secrets are never in the file: env values marked
//! secret, bearer tokens and OAuth credentials live in the login Keychain.
use std::path::{Path, PathBuf};

use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};
use zephium_ipc::work::{WorkServerV1, MAX_WORK_SERVERS};
use zephium_mcp::McpTool;

static SHARED: OnceLock<ConnectionStore> = OnceLock::new();

/// Sets where every profile's connections live; the app does it once at start.
pub fn install(data: &Path) {
    let _ = SHARED.set(ConnectionStore::new(data));
}

/// The app's store, once installed.
pub fn shared() -> Option<&'static ConnectionStore> {
    SHARED.get()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreError {
    Invalid,
    Full,
    Io,
}

/// `<data>/connections/profile-<id>.json`.
#[derive(Clone, Debug)]
pub struct ConnectionStore {
    folder: PathBuf,
    authority: Arc<Mutex<AuthorityRegistry>>,
}

type AuthorityRegistry =
    std::collections::HashMap<(String, String), std::sync::Weak<Mutex<AuthorityState>>>;

#[derive(Debug, Default)]
struct AuthorityState {
    generation: u64,
    changing: usize,
}
impl AuthorityState {
    fn advance(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("connection generation exhausted");
    }
}

/// A session retains only its own authority; registry entries disappear once
/// sessions and in-flight writes release them. No per-server history accumulates.
pub(crate) struct ConnectionGeneration {
    authority: Arc<Mutex<AuthorityState>>,
    generation: u64,
}
impl ConnectionGeneration {
    pub(crate) fn is_current(&self) -> bool {
        let state = self.authority.lock().unwrap_or_else(|p| p.into_inner());
        state.changing == 0 && state.generation == self.generation
    }
}

/// Holds the authority closed through successful or failed credential/configuration writes.
/// Dropping the lease revokes sessions captured before either boundary, including failures.
pub struct ConnectionChange(Arc<Mutex<AuthorityState>>);
impl Drop for ConnectionChange {
    fn drop(&mut self) {
        let mut authority = self.0.lock().unwrap_or_else(|p| p.into_inner());
        authority.advance();
        authority.changing -= 1;
    }
}

fn valid_profile(profile: &str) -> bool {
    !profile.is_empty() && profile.len() <= 64 && profile.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl ConnectionStore {
    pub fn new(data: &Path) -> Self {
        Self {
            folder: data.join("connections"),
            authority: Default::default(),
        }
    }

    fn authority(&self, profile: &str, server: &str) -> Arc<Mutex<AuthorityState>> {
        let mut registry = self.authority.lock().unwrap_or_else(|p| p.into_inner());
        registry.retain(|_, state| state.strong_count() > 0);
        let slot = registry
            .entry((profile.to_owned(), server.to_owned()))
            .or_default();
        if let Some(state) = slot.upgrade() {
            return state;
        }
        let state = Arc::new(Mutex::new(AuthorityState::default()));
        *slot = Arc::downgrade(&state);
        state
    }

    /// Only this profile/server is unavailable while its authority is written.
    pub fn begin_change(&self, profile: &str, server: &str) -> ConnectionChange {
        let state = self.authority(profile, server);
        {
            let mut authority = state.lock().unwrap_or_else(|p| p.into_inner());
            authority.advance();
            authority.changing += 1;
        }
        ConnectionChange(state)
    }

    pub(crate) fn generation(&self, profile: &str, server: &str) -> Option<ConnectionGeneration> {
        let state = self.authority(profile, server);
        let authority = state.lock().unwrap_or_else(|p| p.into_inner());
        (authority.changing == 0).then(|| ConnectionGeneration {
            authority: state.clone(),
            generation: authority.generation,
        })
    }

    /// Browser OAuth waits carry no write lease. Only save/clear revoke sessions.
    pub fn track_tokens(
        &self,
        profile: &str,
        server: &str,
        tokens: Arc<dyn zephium_mcp::oauth::TokenStore>,
    ) -> Arc<dyn zephium_mcp::oauth::TokenStore> {
        Arc::new(TrackedTokens {
            store: self.clone(),
            profile: profile.into(),
            server: server.into(),
            tokens,
        })
    }

    fn path(&self, profile: &str) -> Result<PathBuf, StoreError> {
        if !valid_profile(profile) {
            return Err(StoreError::Invalid);
        }
        Ok(self.folder.join(format!("profile-{profile}.json")))
    }

    /// The profile's servers; a missing or unreadable file is an empty list.
    pub fn servers(&self, profile: &str) -> Result<Vec<WorkServerV1>, StoreError> {
        let path = self.path(profile)?;
        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(Vec::new());
        };
        if bytes.len() > 256 * 1024 {
            return Err(StoreError::Io);
        }
        let file: Value = serde_json::from_slice(&bytes).map_err(|_| StoreError::Io)?;
        if file["version"] != 1 {
            return Err(StoreError::Io);
        }
        let servers: Vec<WorkServerV1> =
            serde_json::from_value(file["servers"].clone()).map_err(|_| StoreError::Io)?;
        Ok(servers
            .into_iter()
            .filter(WorkServerV1::validate)
            .take(MAX_WORK_SERVERS)
            .collect())
    }

    fn write(&self, profile: &str, servers: Vec<WorkServerV1>) -> Result<(), StoreError> {
        let path = self.path(profile)?;
        std::fs::create_dir_all(&self.folder).map_err(|_| StoreError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.folder, std::fs::Permissions::from_mode(0o700));
        }
        let bytes = serde_json::to_vec_pretty(&json!({"version": 1, "servers": servers}))
            .map_err(|_| StoreError::Io)?;
        let temp = path.with_extension(format!("json.{}", std::process::id()));
        std::fs::write(&temp, bytes).map_err(|_| StoreError::Io)?;
        std::fs::rename(&temp, &path).map_err(|_| {
            let _ = std::fs::remove_file(&temp);
            StoreError::Io
        })
    }

    /// Adds `server`, or replaces the one with `previous`'s id (or its own).
    pub fn put(
        &self,
        profile: &str,
        server: WorkServerV1,
        previous: Option<&str>,
    ) -> Result<Vec<WorkServerV1>, StoreError> {
        if !server.validate() {
            return Err(StoreError::Invalid);
        }
        let mut servers = self.servers(profile)?;
        let replaced = previous.unwrap_or(&server.id).to_owned();
        if server.id != replaced && servers.iter().any(|s| s.id == server.id) {
            return Err(StoreError::Invalid);
        }
        let id = server.id.clone();
        match servers.iter().position(|s| s.id == replaced) {
            Some(index) => servers[index] = server,
            None if servers.len() >= MAX_WORK_SERVERS => return Err(StoreError::Full),
            None => servers.push(server),
        }
        let _change = self.begin_change(profile, &id);
        let _previous_change = previous
            .filter(|previous| *previous != id)
            .map(|previous| self.begin_change(profile, previous));
        self.write(profile, servers.clone())?;
        Ok(servers)
    }

    /// Persist a validated server, restoring touched secrets if persistence fails.
    pub fn commit_verified(
        &self,
        profile: &str,
        server: WorkServerV1,
        previous: Option<&str>,
        secrets: &std::collections::HashMap<String, String>,
        vault: &dyn zephium_mcp::keychain::SecretVault,
    ) -> Result<(), StoreError> {
        use zephium_mcp::keychain::KeychainError;
        let mut before = Vec::new();
        for account in secrets.keys() {
            let value = match vault.read(profile, &server.id, account) {
                Ok(value) => Some(value),
                Err(KeychainError::Missing) => None,
                Err(_) => return Err(StoreError::Io),
            };
            before.push((account, value));
        }
        let _change = self.begin_change(profile, &server.id);
        let _previous_change = previous
            .filter(|id| *id != server.id)
            .map(|id| self.begin_change(profile, id));
        let result = (|| {
            for (account, value) in secrets {
                vault
                    .write(profile, &server.id, account, value)
                    .map_err(|_| StoreError::Io)?;
            }
            self.put(profile, server.clone(), previous)?;
            Ok(())
        })();
        if result.is_err() {
            for (account, value) in before {
                let restored = match value {
                    Some(value) => vault.write(profile, &server.id, account, &value),
                    None => vault.delete(profile, &server.id, account),
                };
                if restored.is_err() {
                    return Err(StoreError::Io);
                }
            }
        }
        result
    }

    /// Clear all accounts before removing configuration, preserving a retry on vault failure.
    pub fn remove_with_vault(
        &self,
        profile: &str,
        id: &str,
        vault: &dyn zephium_mcp::keychain::SecretVault,
    ) -> Result<(), StoreError> {
        let _change = self.begin_change(profile, id);
        vault
            .delete_server(profile, id)
            .map_err(|_| StoreError::Io)?;
        self.remove(profile, id)?;
        Ok(())
    }

    pub fn remove(&self, profile: &str, id: &str) -> Result<Vec<WorkServerV1>, StoreError> {
        let _change = self.begin_change(profile, id);
        let mut servers = self.servers(profile)?;
        servers.retain(|s| s.id != id);
        self.write(profile, servers.clone())?;
        let _ = self.put_tools(profile, id, None);
        Ok(servers)
    }

    fn tools_path(&self, profile: &str) -> Result<PathBuf, StoreError> {
        Ok(self.path(profile)?.with_extension("tools.json"))
    }

    /// The tools each server offered when it was last reached, by server id,
    /// so a run can offer them before connecting.
    pub fn tools(&self, profile: &str) -> Vec<(String, Vec<McpTool>)> {
        let Ok(path) = self.tools_path(profile) else {
            return Vec::new();
        };
        let Ok(bytes) = std::fs::read(path) else {
            return Vec::new();
        };
        let Ok(Value::Object(servers)) = serde_json::from_slice::<Value>(&bytes) else {
            return Vec::new();
        };
        servers
            .into_iter()
            .map(|(id, tools)| {
                let tools = tools
                    .as_array()
                    .map(|tools| {
                        tools
                            .iter()
                            .filter_map(|tool| {
                                Some(McpTool {
                                    name: tool["name"].as_str()?.to_owned(),
                                    title: tool["title"].as_str().map(str::to_owned),
                                    description: tool["description"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_owned(),
                                    schema: tool["schema"].clone(),
                                    read_only: tool["read_only"].as_bool(),
                                    destructive: tool["destructive"].as_bool(),
                                })
                            })
                            .take(zephium_mcp::MAX_TOOLS)
                            .collect()
                    })
                    .unwrap_or_default();
                (id, tools)
            })
            .collect()
    }

    /// Keeps (or with `None` forgets) the tools a server offered.
    pub fn put_tools(
        &self,
        profile: &str,
        id: &str,
        tools: Option<&[McpTool]>,
    ) -> Result<(), StoreError> {
        let path = self.tools_path(profile)?;
        let mut all: serde_json::Map<String, Value> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        match tools {
            Some(tools) => {
                all.insert(
                    id.to_owned(),
                    tools
                        .iter()
                        .map(|t| {
                            json!({"name": t.name, "title": t.title, "description": t.description,
                                   "schema": t.schema, "read_only": t.read_only, "destructive": t.destructive})
                        })
                        .collect(),
                );
            }
            None => {
                if all.remove(id).is_none() {
                    return Ok(());
                }
            }
        }
        std::fs::create_dir_all(&self.folder).map_err(|_| StoreError::Io)?;
        let bytes = serde_json::to_vec(&Value::Object(all)).map_err(|_| StoreError::Io)?;
        let temp = path.with_extension(format!("json.{}", std::process::id()));
        std::fs::write(&temp, bytes).map_err(|_| StoreError::Io)?;
        std::fs::rename(&temp, &path).map_err(|_| {
            let _ = std::fs::remove_file(&temp);
            StoreError::Io
        })
    }
}

struct TrackedTokens {
    store: ConnectionStore,
    profile: String,
    server: String,
    tokens: Arc<dyn zephium_mcp::oauth::TokenStore>,
}
impl zephium_mcp::oauth::TokenStore for TrackedTokens {
    fn load(&self) -> Option<String> {
        self.tokens.load()
    }
    fn save(&self, value: &str) -> bool {
        let _change = self.store.begin_change(&self.profile, &self.server);
        self.tokens.save(value)
    }
    fn clear(&self) {
        let _change = self.store.begin_change(&self.profile, &self.server);
        self.tokens.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_ipc::work::{WorkServerAuthV1, WorkServerEnvV1, WorkServerTransportV1};

    fn server(id: &str) -> WorkServerV1 {
        WorkServerV1 {
            id: id.into(),
            name: "Linear".into(),
            transport: WorkServerTransportV1::Http {
                url: "https://mcp.linear.app/mcp".into(),
                auth: WorkServerAuthV1::OAuth,
            },
            enabled: true,
        }
    }

    #[derive(Default)]
    struct MemoryVault(
        std::cell::RefCell<std::collections::HashMap<(String, String), String>>,
        std::cell::Cell<bool>,
    );
    impl zephium_mcp::keychain::SecretVault for MemoryVault {
        fn read(
            &self,
            _: &str,
            server: &str,
            account: &str,
        ) -> Result<String, zephium_mcp::keychain::KeychainError> {
            self.0
                .borrow()
                .get(&(server.into(), account.into()))
                .cloned()
                .ok_or(zephium_mcp::keychain::KeychainError::Missing)
        }
        fn write(
            &self,
            _: &str,
            server: &str,
            account: &str,
            secret: &str,
        ) -> Result<(), zephium_mcp::keychain::KeychainError> {
            self.0
                .borrow_mut()
                .insert((server.into(), account.into()), secret.into());
            Ok(())
        }
        fn delete(
            &self,
            _: &str,
            server: &str,
            account: &str,
        ) -> Result<(), zephium_mcp::keychain::KeychainError> {
            self.0.borrow_mut().remove(&(server.into(), account.into()));
            Ok(())
        }
        fn delete_server(
            &self,
            _: &str,
            server: &str,
        ) -> Result<(), zephium_mcp::keychain::KeychainError> {
            if self.1.get() {
                return Err(zephium_mcp::keychain::KeychainError::Inaccessible);
            }
            self.0.borrow_mut().retain(|(s, _), _| s != server);
            Ok(())
        }
    }

    #[test]
    fn rejected_duplicate_rename_preserves_existing_authority_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::new(dir.path());
        store.put("profile", server("first"), None).unwrap();
        store.put("profile", server("second"), None).unwrap();
        let first = store.generation("profile", "first").unwrap();
        let second = store.generation("profile", "second").unwrap();
        let before = std::fs::read(store.path("profile").unwrap()).unwrap();
        assert_eq!(
            store.put("profile", server("second"), Some("first")),
            Err(StoreError::Invalid)
        );
        assert_eq!(
            std::fs::read(store.path("profile").unwrap()).unwrap(),
            before
        );
        assert!(
            first.is_current(),
            "a rejected rename revoked the unchanged original server"
        );
        assert!(
            second.is_current(),
            "a rejected rename revoked the unchanged destination server"
        );
    }

    #[test]
    fn authority_registry_retains_only_live_session_and_write_keys() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::new(dir.path());
        let live = store.generation("profile", "live").unwrap();
        for index in 0..1000 {
            drop(store.begin_change("profile", &format!("server{index}")));
        }
        assert!(live.is_current());
        assert!(store.authority.lock().unwrap().len() <= 2);
        drop(live);
        drop(store.begin_change("profile", "last"));
        assert_eq!(store.authority.lock().unwrap().len(), 1);
    }

    #[test]
    fn failed_persistence_restores_keys_and_removal_clears_all_accounts() {
        use zephium_mcp::keychain::SecretVault;
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::new(dir.path());
        let vault = MemoryVault::default();
        vault
            .write("profile", "linear", "bearer", "original")
            .unwrap();
        let incoming = [("bearer".into(), "replacement".into())]
            .into_iter()
            .collect();
        assert_eq!(
            store.commit_verified("bad/profile", server("linear"), None, &incoming, &vault),
            Err(StoreError::Invalid)
        );
        assert_eq!(
            vault.read("profile", "linear", "bearer").unwrap(),
            "original"
        );
        store
            .commit_verified("profile", server("linear"), None, &incoming, &vault)
            .unwrap();
        vault
            .write("profile", "linear", "env.OBSOLETE", "old")
            .unwrap();
        vault.1.set(true);
        assert_eq!(
            store.remove_with_vault("profile", "linear", &vault),
            Err(StoreError::Io)
        );
        assert_eq!(store.servers("profile").unwrap().len(), 1);
        vault.1.set(false);
        store
            .remove_with_vault("profile", "linear", &vault)
            .unwrap();
        assert!(vault.0.borrow().is_empty());
        assert!(store.servers("profile").unwrap().is_empty());
    }

    #[test]
    fn servers_round_trip_without_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConnectionStore::new(dir.path());
        let profile = "01M3CTWDG20GFZPACD7905RSRJ";
        assert!(store.servers(profile).unwrap().is_empty());
        store.put(profile, server("linear"), None).unwrap();
        let mut notion = server("notion");
        notion.transport = WorkServerTransportV1::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "@notionhq/notion-mcp-server".into()],
            env: vec![WorkServerEnvV1 {
                name: "NOTION_TOKEN".into(),
                secret: true,
                value: None,
            }],
        };
        store.put(profile, notion.clone(), None).unwrap();
        assert_eq!(store.servers(profile).unwrap().len(), 2);
        let mut renamed = notion.clone();
        renamed.id = "notion-work".into();
        let servers = store.put(profile, renamed, Some("notion")).unwrap();
        assert_eq!(
            servers.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["linear", "notion-work"]
        );
        assert_eq!(
            store.put(profile, server("linear"), Some("notion-work")),
            Err(StoreError::Invalid)
        );
        let mut leaked = notion;
        if let WorkServerTransportV1::Stdio { env, .. } = &mut leaked.transport {
            env[0].value = Some("secret".into());
        }
        assert_eq!(store.put(profile, leaked, None), Err(StoreError::Invalid));
        let text = std::fs::read_to_string(
            dir.path()
                .join(format!("connections/profile-{profile}.json")),
        )
        .unwrap();
        assert!(!text.contains("\"value\": \"secret\""));
        assert_eq!(store.remove(profile, "linear").unwrap().len(), 1);
        assert_eq!(store.servers("../x"), Err(StoreError::Invalid));
    }
}
