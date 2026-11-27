//! Models for the lead agent: the catalog (built in, or Zephium Cloud's when
//! signed in), the person's keys, per-profile choices per role, and
//! [`resolve`], which hands the runtime a ready client.
//!
//! Choices are trusted host state stored through an installed settings port;
//! keys live only in the Keychain. Nothing here logs page, model or person text.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use zephium_agentic::lead::{
    self, keys, models, LeadClient, LeadCloudRecord, LeadCredential, LeadKeyCheck, LeadSecret,
    LeadSecretFuture, LeadStaticCredential, LeadTarget,
};
use zephium_core::ids::ProfileId;
use zephium_core::work::model::*;

use WorkModelProvider as P;

/// The order a role falls back to a provider when nothing is chosen: the
/// person's own keys first, then the signed-in account's hosted models.
const FALLBACK_ORDER: [WorkModelProvider; 11] = [
    P::Anthropic,
    P::OpenAi,
    P::Google,
    P::XAi,
    P::Mistral,
    P::DeepSeek,
    P::Groq,
    P::Cerebras,
    P::OpenRouter,
    P::Compatible,
    P::Cloud,
];
/// Providers that take a key in Settings, in display order.
pub const KEYED_PROVIDERS: [WorkModelProvider; 10] = [
    P::Anthropic,
    P::OpenAi,
    P::Google,
    P::XAi,
    P::Mistral,
    P::DeepSeek,
    P::Groq,
    P::Cerebras,
    P::OpenRouter,
    P::Compatible,
];
const ROLES: [WorkModelRole; 3] = [
    WorkModelRole::Lead,
    WorkModelRole::Page,
    WorkModelRole::Light,
];
const CLOUD_CATALOG_KEY: &str = "work.models.cloud.catalog";
const COMPATIBLE_BASE_KEY: &str = "work.models.compatible.base";
const LEGACY_PRICE_ORIGIN_KEY: &str = "work.models.compatible.legacy_price_origin";
const CLOUD_REFRESH: Duration = Duration::from_secs(60 * 60);
const CLOUD_RETRY: Duration = Duration::from_secs(30);
const MAX_STORED_CATALOG_BYTES: usize = 512 * 1024;

/// App settings, installed by the composition root.
pub trait WorkModelSettings: Send + Sync {
    /// The stored value, if any.
    fn get(&self, key: &str) -> Option<String>;
    /// Stores a value; an empty value clears it.
    fn set(&self, key: String, value: String) -> bool;
}

/// What a hosted call is made for, kept on this computer so the usage history
/// can group calls. The service never learns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkCallSource {
    /// A Work task: the profile that owns it and its id.
    Task {
        profile: String,
        work: String,
    },
    Voice,
    /// One assistant reply, so its charge can be added to the reply.
    Page {
        conversation: String,
        reply: u32,
    },
}

/// A signed-in Zephium Cloud session, installed by the sign-in flow.
pub trait WorkCloudSession: Send + Sync {
    /// Whether a session is active now.
    fn signed_in(&self) -> bool;
    /// The plan's name, as the server reports it.
    fn plan(&self) -> Option<String>;
    /// The API base, such as `https://api.zephium.app`.
    fn base(&self) -> String;
    /// The short-lived bearer, refreshed by the session.
    fn credential(&self) -> Arc<dyn LeadCredential>;
    /// A hosted call ended; `record` says what is known of its charge.
    fn recorded(&self, _record: LeadCloudRecord) {}
    /// The hosted request `request_id` was made for `source`.
    fn sourced(&self, _request_id: &str, _source: &WorkCallSource) {}
}

static SETTINGS: RwLock<Option<Arc<dyn WorkModelSettings>>> = RwLock::new(None);
static CLOUD: RwLock<Option<Arc<dyn WorkCloudSession>>> = RwLock::new(None);
type Observer = Box<dyn Fn() + Send + Sync>;
static OBSERVER: OnceLock<Observer> = OnceLock::new();

/// Installs the settings store. Choices made before it exist only in memory.
pub fn install_settings(settings: Arc<dyn WorkModelSettings>) {
    if let Ok(mut slot) = SETTINGS.write() {
        *slot = Some(settings);
    }
}

/// Installs the Cloud session. Without one, Cloud reads as signed out.
pub fn install_cloud_session(session: Arc<dyn WorkCloudSession>) {
    if let Ok(mut slot) = CLOUD.write() {
        *slot = Some(session);
    }
    notify();
}

/// Called after any change a picker or Settings shows (keys, choices, catalog).
pub fn set_observer(observer: Observer) {
    let _ = OBSERVER.set(observer);
}

/// The account signed in or out. Signed out, the hosted catalog and its
/// cached copy go; signed in, the next refresh fetches it.
pub fn cloud_session_changed() {
    let signed_in = cloud().is_some();
    {
        let mut state = state();
        state.cloud_fetched = None;
        state.cloud_tried = None;
        if !signed_in {
            state.cloud.clear();
            state.cloud_defaults = None;
            state.transcriber = None;
            state.cloud_loaded = true;
        }
    }
    if !signed_in {
        store_setting(CLOUD_CATALOG_KEY.to_owned(), String::new());
    }
    notify();
}

fn notify() {
    if let Some(observer) = OBSERVER.get() {
        observer();
    }
}

pub(crate) fn setting(key: &str) -> Option<String> {
    let settings = SETTINGS.read().ok()?.clone()?;
    settings.get(key).filter(|value| !value.is_empty())
}

pub(crate) fn store_setting(key: String, value: String) -> bool {
    let settings = SETTINGS.read().ok().and_then(|slot| slot.clone());
    settings.is_some_and(|settings| settings.set(key, value))
}

pub(crate) fn cloud() -> Option<Arc<dyn WorkCloudSession>> {
    CLOUD
        .read()
        .ok()?
        .clone()
        .filter(|session| session.signed_in())
}

/// What the person sees about a provider's key. Never the key itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkKeyStatus {
    /// No key stored.
    Missing,
    /// A key is stored and has not been checked yet.
    Set,
    /// The provider accepted the key.
    Valid,
    /// The provider refused the key.
    Invalid,
}

impl WorkKeyStatus {
    fn verdict(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::Missing | Self::Set => "",
        }
    }
}

/// Credential acceptance and service availability are independent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkKeyFault {
    WrongKey,
    Billing,
    RateLimited,
    ProviderDown,
    Offline,
    Keychain,
    /// The Keychain is showing the person a prompt for an earlier call.
    KeychainWaiting,
    Request,
}

fn failure(error: WorkModelError) -> Option<WorkKeyFault> {
    Some(match error {
        WorkModelError::Unauthorized => WorkKeyFault::WrongKey,
        WorkModelError::OverBudget => WorkKeyFault::Billing,
        WorkModelError::RateLimited { .. } => WorkKeyFault::RateLimited,
        WorkModelError::Overloaded => WorkKeyFault::ProviderDown,
        WorkModelError::Network => WorkKeyFault::Offline,
        WorkModelError::BadRequest | WorkModelError::Protocol => WorkKeyFault::Request,
        _ => return None,
    })
}

fn record_observation(
    provider: P,
    generation: u64,
    status: Option<WorkKeyStatus>,
    fault: Option<WorkKeyFault>,
) {
    let changed = {
        let mut state = state();
        if state.generations.get(&provider).copied().unwrap_or(0) != generation {
            return;
        }
        let mut changed = match fault {
            Some(fault) => state.faults.insert(provider, fault) != Some(fault),
            None => state.faults.remove(&provider).is_some(),
        };
        if let Some(status) = status {
            changed |= state.verdicts.insert(provider, status) != Some(status);
            store_setting(key_setting(provider), status.verdict().to_owned());
        }
        changed
    };
    if changed {
        notify();
    }
}

fn record_failure(provider: P, generation: u64, fault: Option<WorkKeyFault>) {
    record_observation(
        provider,
        generation,
        (fault == Some(WorkKeyFault::WrongKey)).then_some(WorkKeyStatus::Invalid),
        fault,
    );
}

pub fn provider_failure(provider: P) -> Option<WorkKeyFault> {
    state().faults.get(&provider).copied()
}

fn generation(provider: P) -> u64 {
    state().generations.get(&provider).copied().unwrap_or(0)
}

fn vault() -> Arc<dyn keys::KeyVault> {
    #[cfg(not(test))]
    {
        Arc::new(keys::SystemVault)
    }
    #[cfg(test)]
    {
        static MEMORY: OnceLock<Arc<tests::MemoryVault>> = OnceLock::new();
        MEMORY.get_or_init(Default::default).clone()
    }
}

fn mutations() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(Default::default)
}

#[derive(Default)]
struct State {
    faults: HashMap<P, WorkKeyFault>,
    generations: HashMap<P, u64>,
    presence: HashMap<WorkModelProvider, bool>,
    verdicts: HashMap<WorkModelProvider, WorkKeyStatus>,
    listed: HashMap<WorkModelProvider, Vec<WorkModelEntry>>,
    cloud: Vec<models::CloudModel>,
    /// The server's per-role defaults; `None` from a server that sends none.
    cloud_defaults: Option<models::CloudDefaults>,
    transcriber: Option<models::CloudTranscriber>,
    cloud_loaded: bool,
    cloud_fetched: Option<Instant>,
    /// When a fetch last started, so concurrent readers share one attempt and a
    /// failing server is not asked again on every read.
    cloud_tried: Option<Instant>,
    /// Choices kept in memory when no settings store is installed.
    choices: HashMap<String, StoredChoice>,
    efforts: HashMap<String, String>,
}

fn state() -> std::sync::MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn key_setting(provider: WorkModelProvider) -> String {
    format!("work.models.key.{}", models::provider_slug(provider))
}

fn choice_key(profile: ProfileId, role: WorkModelRole) -> String {
    let role = match role {
        WorkModelRole::Lead => "lead",
        WorkModelRole::Page => "page",
        WorkModelRole::Light => "light",
        WorkModelRole::Decision => "decision",
    };
    format!("work.models.{profile}.{role}")
}

pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    tokio::task::spawn_blocking(work).await.ok()
}

/// Reads which keys exist, from Keychain attributes only, once per process.
pub(crate) async fn ensure_presence() {
    let missing: Vec<_> = {
        let mut state = state();
        load_cloud_cache(&mut state);
        KEYED_PROVIDERS
            .into_iter()
            .filter(|provider| !state.presence.contains_key(provider))
            .collect()
    };
    if missing.is_empty() {
        return;
    }
    let found = blocking(move || {
        missing
            .into_iter()
            .map(|provider| (provider, vault().present(provider).unwrap_or(false)))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let mut state = state();
    for (provider, present) in found {
        state.presence.entry(provider).or_insert(present);
        let verdict = match setting(&key_setting(provider)).as_deref() {
            Some("valid") => WorkKeyStatus::Valid,
            Some("invalid") => WorkKeyStatus::Invalid,
            _ => WorkKeyStatus::Set,
        };
        state.verdicts.entry(provider).or_insert(verdict);
    }
}

fn key_status(state: &State, provider: WorkModelProvider) -> WorkKeyStatus {
    if !state.presence.get(&provider).copied().unwrap_or(false) {
        return WorkKeyStatus::Missing;
    }
    state
        .verdicts
        .get(&provider)
        .copied()
        .unwrap_or(WorkKeyStatus::Set)
}

fn compatible_base() -> Option<String> {
    setting(COMPATIBLE_BASE_KEY)
}

/// Whether calls to `provider` can be made now.
fn usable(state: &State, provider: WorkModelProvider) -> bool {
    match provider {
        P::Cloud => cloud().is_some() && state.cloud.iter().any(|model| model.available),
        P::Compatible => {
            compatible_base().is_some() && key_status(state, provider) != WorkKeyStatus::Invalid
        }
        provider => {
            state.presence.get(&provider).copied().unwrap_or(false)
                && key_status(state, provider) != WorkKeyStatus::Invalid
        }
    }
}

fn load_cloud_cache(state: &mut State) {
    if state.cloud_loaded {
        return;
    }
    state.cloud_loaded = true;
    if let Some(stored) = setting(CLOUD_CATALOG_KEY) {
        if let Ok(body) = serde_json::from_str(&stored) {
            state.cloud = models::parse_cloud(&body);
            state.cloud_defaults = models::parse_cloud_defaults(&body);
            state.transcriber = models::parse_cloud_transcriber(&body);
        }
    }
}

/// Refreshes the Cloud catalog when signed in and the copy is older than an
/// hour; offline, the cached copy (or the built-in list) stays.
pub async fn refresh_cloud_catalog() {
    let Some(session) = cloud() else { return };
    let stale = {
        let mut state = state();
        load_cloud_cache(&mut state);
        let stale = state
            .cloud_fetched
            .is_none_or(|fetched| fetched.elapsed() > CLOUD_REFRESH)
            && state
                .cloud_tried
                .is_none_or(|tried| tried.elapsed() > CLOUD_RETRY);
        if stale {
            state.cloud_tried = Some(Instant::now());
        }
        stale
    };
    if !stale {
        return;
    }
    let Ok(secret) = session.credential().secret().await else {
        return;
    };
    let Ok(fetched) = lead::fetch_cloud_catalog(&session.base(), &secret).await else {
        return;
    };
    let mut raw: Vec<serde_json::Value> = fetched
        .models
        .iter()
        .map(|model| {
            serde_json::json!({
                "id": model.entry.id.trim_start_matches("zephium/"),
                "provider": models::provider_slug(model.upstream),
                "upstream_model": model.entry.model.model,
                "display_name": model.entry.display_name,
                "description": model.description,
                "tier": model.tier,
                "available": model.available,
                "roles": model.entry.roles,
                "recommended": model.entry.recommended,
                "context_window": model.entry.context_window,
                "max_output": model.entry.max_output,
                "supports": model.entry.supports,
                "plan_min": model.plan_min,
                "limits": {
                    "max_request_bytes": model.max_request_bytes,
                    "max_search_calls": model.max_search_calls,
                    "idle_timeout_ms": model.idle_timeout_ms,
                    "max_duration_ms": model.max_duration_ms,
                },
            })
        })
        .collect();
    if let Some(transcriber) = &fetched.transcriber {
        raw.push(serde_json::json!({
            "id": transcriber.id,
            "provider": "openai",
            "upstream_model": transcriber.upstream_model,
            "roles": ["transcribe"],
            "limits": {"max_request_bytes": transcriber.max_request_bytes},
        }));
    }
    let stored = match &fetched.defaults {
        Some(defaults) => {
            let id = |role: WorkModelRole| {
                defaults
                    .get(role)
                    .map(|id| id.trim_start_matches("zephium/").to_owned())
            };
            serde_json::json!({
                "models": raw,
                "defaults": {
                    "lead": id(WorkModelRole::Lead),
                    "page": id(WorkModelRole::Page),
                    "light": id(WorkModelRole::Light),
                    "decision": id(WorkModelRole::Decision),
                },
            })
        }
        None => serde_json::json!({ "models": raw }),
    };
    // Readers re-read on every change event, and a read asks for this refresh:
    // announcing an unchanged catalog would loop them.
    let changed = match serde_json::to_string(&stored) {
        Ok(text) if text.len() <= MAX_STORED_CATALOG_BYTES => {
            let changed = setting(CLOUD_CATALOG_KEY).as_deref() != Some(text.as_str());
            if changed {
                store_setting(CLOUD_CATALOG_KEY.to_owned(), text);
            }
            changed
        }
        _ => true,
    };
    {
        let mut state = state();
        state.cloud = fetched.models;
        state.transcriber = fetched.transcriber;
        state.cloud_defaults = fetched.defaults;
        state.cloud_fetched = Some(Instant::now());
    }
    if changed {
        notify();
    }
}

/// Curated BYOK entries, custom endpoint models, and the signed-in account's
/// hosted models.
fn entries(state: &State) -> Vec<WorkModelEntry> {
    let mut all = Vec::new();
    all.extend(models::builtin().into_iter().filter(|entry| {
        state
            .listed
            .get(&entry.model.provider)
            .is_none_or(|listed| listed.iter().any(|item| item.id == entry.id))
    }));
    let local = compatible_base().is_some_and(|base| is_local(&base));
    for provider in [P::Compatible] {
        for entry in state.listed.get(&provider).into_iter().flatten() {
            if !all.iter().any(|known| known.id == entry.id) {
                all.push(priced_locally(entry.clone(), local));
            }
        }
    }
    if cloud().is_some() {
        all.extend(state.cloud.iter().map(|model| model.entry.clone()));
    }
    all
}

// Persist pricing provenance privately alongside the existing choice fields.
// Absent metadata denotes legacy choices whose zero price may have been local-derived.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct StoredChoice {
    #[serde(flatten)]
    entry: WorkModelEntry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_price: Option<bool>,
    /// The endpoint whose listing supplied this price; absent for legacy choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pricing_endpoint: Option<String>,
}

fn stored_choice(state: &State, profile: ProfileId, role: WorkModelRole) -> Option<WorkModelEntry> {
    let key = choice_key(profile, role);
    let mut stored: StoredChoice = match setting(&key) {
        Some(text) => serde_json::from_str(&text).ok()?,
        None => state.choices.get(&key).cloned()?,
    };
    let endpoint = compatible_base().map(|base| base.trim().trim_end_matches('/').to_owned());
    let local = endpoint.as_ref().is_some_and(|base| is_local(base));
    if stored.entry.model.provider == P::Compatible && stored.pricing_endpoint.is_none() {
        // Old local_price described the endpoint class, not whether a price
        // was synthesized. The immutable anchor binds even unopened profiles;
        // resolving a saved choice never needs to write its setting.
        let origin = setting(LEGACY_PRICE_ORIGIN_KEY)
            .map(|text| serde_json::from_str::<Option<String>>(&text).unwrap_or_default())
            .unwrap_or_else(|| endpoint.clone());
        if let (Some(was_local), Some(origin)) = (stored.local_price, origin.as_ref()) {
            if was_local != is_local(origin) {
                stored.entry.price = None;
            }
        }
        stored.pricing_endpoint = Some(origin.unwrap_or_default());
    }
    // A current listing is authoritative, including an explicit remote zero price.
    let entry = entries(state)
        .into_iter()
        .chain(state.listed.values().flatten().cloned())
        .find(|entry| entry.id == stored.entry.id)
        .unwrap_or_else(|| {
            // Keep an unavailable choice, but do not carry local-derived or
            // unclassified legacy zero pricing to a remote endpoint.
            let mut entry = stored.entry;
            if entry.model.provider == P::Compatible {
                let changed_endpoint = stored.pricing_endpoint.as_ref().is_some_and(|origin| {
                    origin.as_str() != endpoint.as_deref().unwrap_or_default()
                });
                let unproven_zero =
                    !local && entry.is_free_compatible() && stored.local_price != Some(false);
                if changed_endpoint || unproven_zero {
                    entry.price = None;
                }
            }
            entry
        });
    Some(priced_locally(entry, local))
}

fn is_local(base: &str) -> bool {
    zephium_core::work::model::local_model_endpoint(base)
}

/// Unpriced local models are free; explicit rates remain authoritative.
fn priced_locally(mut entry: WorkModelEntry, local: bool) -> WorkModelEntry {
    if local && entry.model.provider == P::Compatible && entry.price.is_none() {
        entry.price = Some(WorkModelPrice {
            input: 0,
            cached_input: 0,
            output: 0,
        });
    }
    entry
}

fn family_entry(
    state: &State,
    provider: WorkModelProvider,
    role: WorkModelRole,
) -> Option<WorkModelEntry> {
    if provider == P::Cloud {
        let hosted = || {
            state
                .cloud
                .iter()
                .filter(|model| model.available)
                .map(|model| &model.entry)
        };
        // The server's default wins; a role it leaves empty is the person's
        // to choose, never filled with a guess.
        if let Some(defaults) = &state.cloud_defaults {
            let id = defaults.get(role)?;
            return hosted().find(|entry| entry.id == id).cloned();
        }
        return hosted()
            .find(|entry| entry.recommended && entry.roles.contains(&role))
            .or_else(|| hosted().find(|entry| entry.roles.contains(&role)))
            .or_else(|| hosted().find(|entry| entry.roles.contains(&WorkModelRole::Lead)))
            .cloned();
    }
    let offered = |entry: &&WorkModelEntry| {
        entry.model.provider == provider
            && state
                .listed
                .get(&provider)
                .is_none_or(|listed| listed.iter().any(|item| item.id == entry.id))
    };
    let catalog = models::builtin();
    let preferred = models::family_default(provider, role);
    catalog
        .iter()
        .filter(offered)
        .find(|entry| Some(entry.model.model.as_str()) == preferred)
        .or_else(|| {
            catalog
                .iter()
                .filter(offered)
                .find(|entry| entry.roles.contains(&role))
        })
        .or_else(|| {
            catalog
                .iter()
                .filter(offered)
                .find(|entry| entry.roles.contains(&WorkModelRole::Lead))
        })
        .cloned()
        .or_else(|| {
            state
                .listed
                .get(&provider)?
                .iter()
                .find(|entry| {
                    entry.roles.contains(&role) || entry.roles.contains(&WorkModelRole::Lead)
                })
                .cloned()
                .map(|entry| {
                    priced_locally(
                        entry,
                        provider == P::Compatible
                            && compatible_base().is_some_and(|base| is_local(&base)),
                    )
                })
        })
}

/// The model a role runs with: the person's choice when its provider is
/// usable, else the lead's provider family, else the first keyed provider.
fn effective(state: &State, profile: ProfileId, role: WorkModelRole) -> Option<WorkModelEntry> {
    if let Some(choice) = stored_choice(state, profile, role) {
        let exists = if choice.model.provider == P::Cloud {
            state
                .cloud
                .iter()
                .any(|model| model.entry.id == choice.id && model.available)
        } else {
            state
                .listed
                .get(&choice.model.provider)
                .is_none_or(|entries| entries.iter().any(|entry| entry.id == choice.id))
        };
        return (usable(state, choice.model.provider) && exists).then_some(choice);
    }
    if role != WorkModelRole::Lead {
        if let Some(lead) = effective(state, profile, WorkModelRole::Lead) {
            return family_entry(state, lead.model.provider, role).or(Some(lead));
        }
    }
    FALLBACK_ORDER
        .into_iter()
        .filter(|provider| usable(state, *provider))
        .find_map(|provider| family_entry(state, provider, role))
}

fn effort_key(profile: ProfileId) -> String {
    format!("work.models.{profile}.lead.effort")
}

fn effort_name(effort: WorkModelReasoning) -> &'static str {
    match effort {
        WorkModelReasoning::Low => "low",
        WorkModelReasoning::Medium => "medium",
        WorkModelReasoning::High => "high",
        WorkModelReasoning::XHigh => "xhigh",
        WorkModelReasoning::Max => "max",
    }
}

fn stored_effort(state: &State, profile: ProfileId) -> Option<WorkModelReasoning> {
    let key = effort_key(profile);
    let name = setting(&key).or_else(|| state.efforts.get(&key).cloned())?;
    [
        WorkModelReasoning::Low,
        WorkModelReasoning::Medium,
        WorkModelReasoning::High,
        WorkModelReasoning::XHigh,
        WorkModelReasoning::Max,
    ]
    .into_iter()
    .find(|effort| effort_name(*effort) == name)
}

/// The provider whose wire `entry` travels: a hosted model's upstream.
fn upstream(state: &State, entry: &WorkModelEntry) -> Option<WorkModelProvider> {
    match entry.model.provider {
        P::Cloud => state
            .cloud
            .iter()
            .find(|model| model.entry.id == entry.id)
            .map(|model| model.upstream),
        provider => Some(provider),
    }
}

/// The efforts `entry` takes as a lead, weakest first; empty when it has no
/// reasoning its wire can set.
fn efforts(state: &State, entry: &WorkModelEntry) -> Vec<WorkModelReasoning> {
    match upstream(state, entry) {
        Some(upstream) if entry.supports.reasoning => models::efforts(upstream, &entry.model.model),
        _ => Vec::new(),
    }
}

/// The effort the lead runs `entry` at for `profile`: the person's choice,
/// nearest to what the model takes, else the model's default.
fn effort_for(
    state: &State,
    profile: ProfileId,
    entry: &WorkModelEntry,
) -> Option<WorkModelReasoning> {
    let levels = efforts(state, entry);
    match stored_effort(state, profile) {
        Some(chosen) => models::nearest_effort(&levels, chosen),
        None => models::default_effort(upstream(state, entry)?, &entry.model.model, &levels),
    }
}

/// The lead's effort for one run in `profile` on `entry`; read once when a
/// run starts, so a change applies to the next run.
pub fn lead_effort(profile: ProfileId, entry: &WorkModelEntry) -> Option<WorkModelReasoning> {
    effort_for(&state(), profile, entry)
}

/// Stores the lead's effort for `profile`; `None` returns it to each model's default.
pub fn set_effort(
    profile: ProfileId,
    effort: Option<WorkModelReasoning>,
) -> Result<(), WorkModelError> {
    let key = effort_key(profile);
    let value = effort.map(effort_name).unwrap_or_default().to_owned();
    if SETTINGS.read().ok().is_some_and(|slot| slot.is_some())
        && !store_setting(key.clone(), value.clone())
    {
        return Err(WorkModelError::Unauthorized);
    }
    {
        let mut state = state();
        if value.is_empty() {
            state.efforts.remove(&key);
        } else {
            state.efforts.insert(key, value);
        }
    }
    notify();
    Ok(())
}

/// Tracks the key's standing from real calls: a refusal marks it invalid,
/// success marks it valid, so Settings and the picker stay truthful.
struct Observed {
    inner: LeadClient,
    provider: WorkModelProvider,
    generation: u64,
}

impl WorkModelClient for Observed {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        events: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        Box::pin(async move {
            let result = self.inner.call(request, events).await;
            if KEYED_PROVIDERS.contains(&self.provider)
                && generation(self.provider) == self.generation
            {
                match &result {
                    Ok(_) => {
                        record_observation(
                            self.provider,
                            self.generation,
                            Some(WorkKeyStatus::Valid),
                            None,
                        );
                    }
                    Err(error) => {
                        if let Some(fault) = failure(*error) {
                            record_failure(self.provider, self.generation, Some(fault));
                        }
                    }
                }
            }
            result
        })
    }
}

struct VaultCredential {
    provider: P,
    generation: u64,
}
impl LeadCredential for VaultCredential {
    fn secret(&self) -> LeadSecretFuture<'_> {
        Box::pin(async move {
            let _guard = mutations().lock().await;
            let provider = self.provider;
            if generation(provider) != self.generation {
                return Err(WorkModelError::Unauthorized);
            }
            match blocking(move || vault().load(provider)).await {
                Some(Ok(secret)) => Ok(secret),
                Some(Err(keys::LeadKeyError::Missing)) => Err(WorkModelError::MissingKey),
                Some(Err(keys::LeadKeyError::AwaitingApproval)) => {
                    record_failure(
                        provider,
                        self.generation,
                        Some(WorkKeyFault::KeychainWaiting),
                    );
                    Err(WorkModelError::MissingKey)
                }
                _ => {
                    record_failure(provider, self.generation, Some(WorkKeyFault::Keychain));
                    // A vault refusal is not a provider authentication failure.
                    Err(WorkModelError::MissingKey)
                }
            }
        })
    }
    fn rejected(&self) {}
}

/// A local endpoint may need no key; it then gets a placeholder bearer.
struct CompatibleCredential {
    generation: u64,
}

impl CompatibleCredential {
    async fn load(
        &self,
        _guard: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<Arc<LeadSecret>, WorkModelError> {
        if generation(P::Compatible) != self.generation {
            return Err(WorkModelError::Unauthorized);
        }
        let stored = blocking(|| vault().load(P::Compatible)).await;
        match stored {
            Some(Ok(secret)) => Ok(secret),
            Some(Err(keys::LeadKeyError::Missing)) => {
                LeadSecret::new("zephium".into()).map(Arc::new)
            }
            Some(Err(keys::LeadKeyError::AwaitingApproval)) => {
                record_failure(
                    P::Compatible,
                    self.generation,
                    Some(WorkKeyFault::KeychainWaiting),
                );
                Err(WorkModelError::MissingKey)
            }
            _ => {
                record_failure(P::Compatible, self.generation, Some(WorkKeyFault::Keychain));
                Err(WorkModelError::MissingKey)
            }
        }
    }
}

impl LeadCredential for CompatibleCredential {
    fn secret(&self) -> LeadSecretFuture<'_> {
        Box::pin(async move {
            let _guard = mutations().lock().await;
            self.load(&_guard).await
        })
    }
    fn rejected(&self) {}
}

/// Whether the person's own key for `provider` can be used now.
pub(crate) async fn key_usable(provider: WorkModelProvider) -> bool {
    ensure_presence().await;
    let state = state();
    provider != P::Cloud && usable(&state, provider)
}

/// The key for `provider`, read from the vault for one call.
pub(crate) fn key_credential(provider: WorkModelProvider) -> Arc<dyn LeadCredential> {
    Arc::new(VaultCredential {
        provider,
        generation: generation(provider),
    })
}

/// The signed-in session and its hosted speech model, when both exist.
pub(crate) async fn hosted_transcriber(
) -> Option<(Arc<dyn WorkCloudSession>, models::CloudTranscriber)> {
    let session = cloud()?;
    ensure_presence().await;
    let transcriber = state().transcriber.clone()?;
    Some((session, transcriber))
}

/// A provider refused its key during a call outside the model clients.
pub(crate) fn key_refused(provider: WorkModelProvider) {
    record_failure(provider, generation(provider), Some(WorkKeyFault::WrongKey));
}

#[cfg(test)]
fn client(
    state: &State,
    entry: &WorkModelEntry,
) -> Result<Arc<dyn WorkModelClient>, WorkModelError> {
    client_for(state, entry, None)
}

fn client_for(
    state: &State,
    entry: &WorkModelEntry,
    source: Option<WorkCallSource>,
) -> Result<Arc<dyn WorkModelClient>, WorkModelError> {
    let provider = entry.model.provider;
    let (target, credential): (LeadTarget, Arc<dyn LeadCredential>) = match provider {
        P::Cloud => {
            let session = cloud().ok_or(WorkModelError::MissingKey)?;
            // A locked model is listed to explain the plan, never called.
            let hosted = state
                .cloud
                .iter()
                .find(|model| model.entry.id == entry.id && model.available)
                .ok_or(WorkModelError::BadRequest)?;
            let target = LeadTarget::cloud(&session.base(), hosted.upstream)?;
            if target.wire() != entry.model.wire {
                return Err(WorkModelError::BadRequest);
            }
            let observer = session.clone();
            let mut client = LeadClient::new(target, session.credential(), entry.price.clone())
                .with_cloud_observer(Arc::new(move |record| {
                    if let (Some(id), Some(source)) = (&record.request_id, &source) {
                        observer.sourced(id, source);
                    }
                    observer.recorded(record);
                }));
            if let Some(max) = hosted.max_request_bytes {
                client = client.with_request_limit(max);
            }
            if let Some(calls) = hosted.max_search_calls {
                client = client.with_search_limit(calls);
            }
            client = client.with_call_limits(hosted.idle_timeout_ms, hosted.max_duration_ms);
            return Ok(Arc::new(Observed {
                inner: client,
                provider,
                generation: state.generations.get(&provider).copied().unwrap_or(0),
            }));
        }
        P::Compatible => (
            LeadTarget::compatible(&compatible_base().ok_or(WorkModelError::MissingKey)?)?,
            Arc::new(CompatibleCredential {
                generation: state.generations.get(&provider).copied().unwrap_or(0),
            }),
        ),
        provider => (
            LeadTarget::direct(provider)?,
            Arc::new(VaultCredential {
                provider,
                generation: state.generations.get(&provider).copied().unwrap_or(0),
            }),
        ),
    };
    if target.wire() != entry.model.wire {
        return Err(WorkModelError::BadRequest);
    }
    Ok(Arc::new(Observed {
        inner: LeadClient::new(target, credential, entry.price.clone()),
        provider,
        generation: state.generations.get(&provider).copied().unwrap_or(0),
    }))
}

/// The model and a ready client for `role` in `profile`.
pub async fn resolve(
    profile: ProfileId,
    role: WorkModelRole,
) -> Result<(WorkModelRef, Arc<dyn WorkModelClient>), WorkModelError> {
    let (entry, client) = resolve_entry(profile, role).await?;
    Ok((entry.model, client))
}

/// Like [`resolve`], with the catalog entry (limits, supports, price).
pub async fn resolve_entry(
    profile: ProfileId,
    role: WorkModelRole,
) -> Result<(WorkModelEntry, Arc<dyn WorkModelClient>), WorkModelError> {
    resolve_entry_for(profile, role, None).await
}

/// Like [`resolve_entry`], with the task a hosted model's calls are made for,
/// so its request ids can be grouped by task in the usage history.
pub async fn resolve_entry_for(
    profile: ProfileId,
    role: WorkModelRole,
    source: Option<WorkCallSource>,
) -> Result<(WorkModelEntry, Arc<dyn WorkModelClient>), WorkModelError> {
    ensure_presence().await;
    let state = state();
    let entry = effective(&state, profile, role).ok_or(WorkModelError::MissingKey)?;
    let client = client_for(&state, &entry, source)?;
    Ok((entry, client))
}

/// One provider as Settings and the picker show it.
#[derive(Clone, Debug)]
pub struct WorkProviderView {
    pub provider: WorkModelProvider,
    pub key: WorkKeyStatus,
    pub fault: Option<WorkKeyFault>,
    /// The configured base URL, for the OpenAI-compatible endpoint only.
    pub base: Option<String>,
}

/// Everything the picker and Settings → AI render.
#[derive(Clone, Debug)]
pub struct WorkModelsView {
    pub entries: Vec<WorkModelEntry>,
    /// What the person chose per role (lead, page, light), if anything.
    pub chosen: [Option<String>; 3],
    /// What each role runs with now.
    pub effective: [Option<String>; 3],
    pub providers: Vec<WorkProviderView>,
    pub cloud_signed_in: bool,
    pub cloud_plan: Option<String>,
    /// Hosted models listed for this account but locked by its plan.
    pub cloud_locked: Vec<String>,
    /// The cheapest plan each locked model needs, when the server says.
    pub cloud_locked_plans: Vec<(String, Option<String>)>,
    /// The efforts each entry takes as a lead, for entries that take any.
    pub efforts: Vec<(String, Vec<WorkModelReasoning>)>,
    /// The effort the lead runs at now; `None` when its model takes none.
    pub effort: Option<WorkModelReasoning>,
}

/// The current view for `profile`.
pub async fn view(profile: ProfileId) -> WorkModelsView {
    ensure_presence().await;
    let state = state();
    let chosen = ROLES.map(|role| stored_choice(&state, profile, role).map(|entry| entry.id));
    let effort = effective(&state, profile, WorkModelRole::Lead)
        .and_then(|lead| effort_for(&state, profile, &lead));
    let effective = ROLES.map(|role| effective(&state, profile, role).map(|entry| entry.id));
    let mut entries = entries(&state);
    for role in ROLES {
        if let Some(choice) = stored_choice(&state, profile, role) {
            if !entries.iter().any(|entry| entry.id == choice.id) {
                entries.push(choice);
            }
        }
    }
    let efforts = entries
        .iter()
        .map(|entry| (entry.id.clone(), efforts(&state, entry)))
        .filter(|(_, levels)| !levels.is_empty())
        .collect();
    let providers = KEYED_PROVIDERS
        .into_iter()
        .map(|provider| WorkProviderView {
            provider,
            key: key_status(&state, provider),
            fault: state.faults.get(&provider).copied(),
            base: (provider == P::Compatible).then(compatible_base).flatten(),
        })
        .collect();
    WorkModelsView {
        entries,
        chosen,
        effective,
        providers,
        cloud_signed_in: cloud().is_some(),
        cloud_plan: cloud().and_then(|session| session.plan()),
        cloud_locked_plans: state
            .cloud
            .iter()
            .filter(|model| !model.available)
            .map(|model| (model.entry.id.clone(), model.plan_min.clone()))
            .collect(),
        cloud_locked: state
            .cloud
            .iter()
            .filter(|model| !model.available)
            .map(|model| model.entry.id.clone())
            .collect(),
        efforts,
        effort,
    }
}

/// Stores the person's choice for `role`; `None` returns it to the default.
pub fn choose(
    profile: ProfileId,
    role: WorkModelRole,
    id: Option<&str>,
) -> Result<(), WorkModelError> {
    if role == WorkModelRole::Decision {
        return Err(WorkModelError::BadRequest);
    }
    let key = choice_key(profile, role);
    let value = match id {
        None => String::new(),
        Some(id) => {
            let state = state();
            let entry = entries(&state)
                .into_iter()
                .chain(state.listed.values().flatten().cloned())
                .find(|entry| entry.id == id)
                .ok_or(WorkModelError::BadRequest)?;
            let locked = state
                .cloud
                .iter()
                .any(|model| model.entry.id == id && !model.available);
            if locked {
                return Err(WorkModelError::BadRequest);
            }
            if !entry.roles.contains(&role) && !entry.roles.contains(&WorkModelRole::Lead) {
                return Err(WorkModelError::BadRequest);
            }
            let pricing_endpoint = (entry.model.provider == P::Compatible)
                .then(compatible_base)
                .flatten()
                .map(|base| base.trim().trim_end_matches('/').to_owned());
            let local_price = (entry.model.provider == P::Compatible).then(|| {
                pricing_endpoint.as_ref().is_some_and(|base| is_local(base))
                    && state
                        .listed
                        .get(&P::Compatible)
                        .into_iter()
                        .flatten()
                        .find(|listed| listed.id == entry.id)
                        .is_some_and(|listed| listed.price.is_none())
            });
            serde_json::to_string(&StoredChoice {
                entry,
                local_price,
                pricing_endpoint,
            })
            .map_err(|_| WorkModelError::BadRequest)?
        }
    };
    if SETTINGS.read().ok().is_some_and(|slot| slot.is_some())
        && !store_setting(key.clone(), value.clone())
    {
        return Err(WorkModelError::Unauthorized);
    }
    {
        let mut state = state();
        if value.is_empty() {
            state.choices.remove(&key);
        } else if let Ok(entry) = serde_json::from_str(&value) {
            state.choices.insert(key.clone(), entry);
        }
    }
    notify();
    Ok(())
}

fn check_target(provider: WorkModelProvider) -> Result<LeadTarget, WorkModelError> {
    match provider {
        P::Compatible => {
            LeadTarget::compatible(&compatible_base().ok_or(WorkModelError::BadRequest)?)
        }
        P::Cloud => Err(WorkModelError::BadRequest),
        provider => LeadTarget::direct(provider),
    }
}

fn apply_check(provider: P, check: LeadKeyCheck) -> WorkKeyStatus {
    let (status, fault) = match check {
        LeadKeyCheck::Valid(entries) => {
            if provider != P::OpenRouter {
                state().listed.insert(provider, entries);
            }
            (WorkKeyStatus::Valid, None)
        }
        LeadKeyCheck::Invalid => (WorkKeyStatus::Invalid, Some(WorkKeyFault::WrongKey)),
        LeadKeyCheck::Billing => (key_status(&state(), provider), Some(WorkKeyFault::Billing)),
        LeadKeyCheck::RateLimited => (
            key_status(&state(), provider),
            Some(WorkKeyFault::RateLimited),
        ),
        LeadKeyCheck::ProviderDown => (
            key_status(&state(), provider),
            Some(WorkKeyFault::ProviderDown),
        ),
        LeadKeyCheck::Unreachable => (key_status(&state(), provider), Some(WorkKeyFault::Offline)),
        LeadKeyCheck::Failed => (key_status(&state(), provider), Some(WorkKeyFault::Request)),
    };
    record_observation(provider, generation(provider), Some(status), fault);
    status
}

/// Check without paid inference. Store even a refused key so its row stays truthful.
pub async fn set_key(provider: P, secret: String) -> Result<WorkKeyStatus, WorkModelError> {
    let _guard = mutations().lock().await;
    if !KEYED_PROVIDERS.contains(&provider) {
        return Err(WorkModelError::BadRequest);
    }
    let target = check_target(provider)?;
    let secret = LeadSecret::new(secret)?;
    let verdict = lead::check_key(&target, &secret).await;
    blocking(move || vault().store(provider, secret))
        .await
        .ok_or(WorkModelError::Unauthorized)?
        .map_err(|_| WorkModelError::Unauthorized)?;
    {
        let mut state = state();
        *state.generations.entry(provider).or_default() += 1;
        state.presence.insert(provider, true);
        state.verdicts.insert(provider, WorkKeyStatus::Set);
        state.listed.remove(&provider);
    }
    let status = apply_check(provider, verdict);
    notify();
    Ok(status)
}

pub async fn test_key(provider: P) -> Result<WorkKeyStatus, WorkModelError> {
    let _guard = mutations().lock().await;
    let secret = match blocking(move || vault().load(provider)).await {
        Some(Ok(secret)) => secret,
        Some(Err(keys::LeadKeyError::Missing)) => return Ok(WorkKeyStatus::Missing),
        Some(Err(keys::LeadKeyError::AwaitingApproval)) => {
            record_failure(
                provider,
                generation(provider),
                Some(WorkKeyFault::KeychainWaiting),
            );
            return Err(WorkModelError::MissingKey);
        }
        _ => {
            record_failure(provider, generation(provider), Some(WorkKeyFault::Keychain));
            return Err(WorkModelError::Unauthorized);
        }
    };
    Ok(apply_check(
        provider,
        lead::check_key(&check_target(provider)?, &secret).await,
    ))
}

/// Persist defaults only into empty roles after the first accepted key in this profile.
pub fn fill_defaults(profile: ProfileId, provider: P) {
    if key_status(&state(), provider) != WorkKeyStatus::Valid {
        return;
    }
    for role in ROLES {
        let entry = {
            let state = state();
            if stored_choice(&state, profile, role).is_some() {
                continue;
            }
            family_entry(&state, provider, role)
        };
        if let Some(entry) = entry {
            let _ = choose(profile, role, Some(&entry.id));
        }
    }
}

/// Lead availability alone gates Work; page and light can fall back to the lead.
pub async fn ready(profile: ProfileId) -> bool {
    ensure_presence().await;
    effective(&state(), profile, WorkModelRole::Lead).is_some()
}

/// The provider search a run in `profile` would fall back to when its
/// browser search cannot use a results page, in the order a run picks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkSearchReadiness {
    /// A chosen model's own provider search, light first.
    Native(WorkModelEntry),
    /// The fixed public search with the person's OpenAI key.
    OpenAi,
    /// No chosen model can search and no OpenAI key is set.
    Unavailable,
}

/// Mirrors the run's own choice of fallback search.
pub async fn search_readiness(profile: ProfileId) -> WorkSearchReadiness {
    ensure_presence().await;
    let state = state();
    let Some(lead) = effective(&state, profile, WorkModelRole::Lead) else {
        return WorkSearchReadiness::Unavailable;
    };
    let page = effective(&state, profile, WorkModelRole::Page).unwrap_or_else(|| lead.clone());
    let light = effective(&state, profile, WorkModelRole::Light).unwrap_or_else(|| page.clone());
    if let Some(entry) = [light, lead, page]
        .into_iter()
        .find(|entry| entry.supports.native_search)
    {
        return WorkSearchReadiness::Native(entry);
    }
    if usable(&state, P::OpenAi) {
        return WorkSearchReadiness::OpenAi;
    }
    WorkSearchReadiness::Unavailable
}

/// Removes the key, preserving explicit role choices for the person to replace.
pub async fn clear_key(provider: WorkModelProvider) -> Result<(), WorkModelError> {
    let _guard = mutations().lock().await;
    if !KEYED_PROVIDERS.contains(&provider) {
        return Err(WorkModelError::BadRequest);
    }
    blocking(move || vault().clear(provider))
        .await
        .ok_or(WorkModelError::Unauthorized)?
        .map_err(|_| WorkModelError::Unauthorized)?;
    {
        let mut state = state();
        *state.generations.entry(provider).or_default() += 1;
        state.faults.remove(&provider);
        state.presence.insert(provider, false);
        state.verdicts.remove(&provider);
        state.listed.remove(&provider);
    }
    store_setting(key_setting(provider), String::new());
    notify();
    Ok(())
}

/// Sets or clears the OpenAI-compatible endpoint's base URL.
pub async fn set_compatible_base(base: Option<&str>) -> Result<(), WorkModelError> {
    let _guard = mutations().lock().await;
    let value = match base.map(str::trim).filter(|base| !base.is_empty()) {
        Some(base) => {
            LeadTarget::compatible(base)?;
            base.trim_end_matches('/').to_owned()
        }
        None => String::new(),
    };
    let changed = {
        let _state = state();
        let changed = compatible_base().as_deref().unwrap_or_default() != value;
        // A failed anchor must leave the old endpoint and credential usable.
        if changed && setting(LEGACY_PRICE_ORIGIN_KEY).is_none() {
            let origin = compatible_base().map(|base| base.trim().trim_end_matches('/').to_owned());
            let origin = serde_json::to_string(&origin).map_err(|_| WorkModelError::BadRequest)?;
            if !store_setting(LEGACY_PRICE_ORIGIN_KEY.to_owned(), origin) {
                return Err(WorkModelError::Unauthorized);
            }
        }
        changed
    };
    if changed {
        blocking(|| vault().clear(P::Compatible))
            .await
            .ok_or(WorkModelError::Unauthorized)?
            .map_err(|_| WorkModelError::Unauthorized)?;
    }
    let mut state = state();
    // Preserve a committed anchor even if the base write fails.
    if !store_setting(COMPATIBLE_BASE_KEY.to_owned(), value) {
        return Err(WorkModelError::Unauthorized);
    }
    if changed {
        *state.generations.entry(P::Compatible).or_default() += 1;
        state.presence.insert(P::Compatible, false);
        state.verdicts.remove(&P::Compatible);
        state.faults.remove(&P::Compatible);
        state.listed.remove(&P::Compatible);
    }
    drop(state);
    notify();
    Ok(())
}

/// The provider's own model list for "More models", kept for this session.
pub async fn more_models(
    provider: WorkModelProvider,
) -> Result<Vec<WorkModelEntry>, WorkModelError> {
    let _guard = mutations().lock().await;
    let current_generation = generation(provider);
    if let Some(listed) = state().listed.get(&provider) {
        return Ok(listed.clone());
    }
    let target = check_target(provider)?;
    let secret: Arc<LeadSecret> = if provider == P::Compatible {
        CompatibleCredential {
            generation: current_generation,
        }
        .load(&_guard)
        .await?
    } else {
        match blocking(move || vault().load(provider)).await {
            Some(Ok(secret)) => secret,
            Some(Err(keys::LeadKeyError::Missing | keys::LeadKeyError::AwaitingApproval)) => {
                return Err(WorkModelError::MissingKey)
            }
            _ => return Err(WorkModelError::Unauthorized),
        }
    };
    let listed = lead::list_models(&target, &secret).await;
    if let Err(error) = &listed {
        if let Some(fault) = failure(*error) {
            record_failure(provider, current_generation, Some(fault));
        }
    }
    let listed = listed?;
    state().listed.insert(provider, listed.clone());
    notify();
    Ok(listed)
}

#[doc(hidden)]
pub fn cloud_session_for_tests(session: Option<Arc<dyn WorkCloudSession>>) {
    if let Ok(mut slot) = CLOUD.write() {
        *slot = session;
    }
}

#[doc(hidden)]
pub fn static_credential(secret: LeadSecret) -> Arc<dyn LeadCredential> {
    Arc::new(LeadStaticCredential::new(secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_pricing_preserves_explicit_rates_and_defaults_only_unknown_prices() {
        for base in [
            "http://localhost:11434/v1",
            "https://localhost/v1",
            "https://127.0.0.2/v1",
            "https://192.168.1.5/v1",
            "https://10.2.3.4/v1",
            "https://172.16.1.2/v1",
            "https://[fd12::1]/v1",
            "https://lmstudio.local/v1",
        ] {
            assert!(is_local(base), "{base}");
        }
        for base in [
            "https://172.32.1.2/v1",
            "https://localhost.example/v1",
            "https://192.168.1.5.example/v1",
            "ftp://localhost/v1",
        ] {
            assert!(!is_local(base), "{base}");
        }
        assert!(is_local("http://127.0.0.1:1234/v1"));
        assert!(is_local("http://[::1]:11434"));
        assert!(!is_local("http://localhost.example.com/v1"));
        assert!(!is_local("https://llm.example.com/v1"));
        let entry = models::builtin()
            .into_iter()
            .next()
            .map(|entry| WorkModelEntry {
                model: WorkModelRef {
                    provider: P::Compatible,
                    ..entry.model.clone()
                },
                price: None,
                ..entry
            })
            .unwrap();
        let resolved = priced_locally(entry.clone(), true);
        assert!(
            priced_locally(resolved.clone(), false).is_free_compatible(),
            "an explicit zero price from a remote model list must survive"
        );
        let free = resolved.price.unwrap();
        assert_eq!((free.input, free.cached_input, free.output), (0, 0, 0));
        assert!(priced_locally(entry.clone(), false).price.is_none());
        let priced = WorkModelEntry {
            price: Some(WorkModelPrice {
                input: 10,
                cached_input: 5,
                output: 20,
            }),
            ..entry
        };
        for base in [
            "http://localhost:11434/v1",
            "https://192.168.1.5/v1",
            "http://100.64.1.2/v1",
            "http://host.docker.internal/v1",
            "https://lmstudio.local/v1",
            "https://models.example.com/v1",
        ] {
            let price = priced_locally(priced.clone(), is_local(base))
                .price
                .unwrap();
            assert_eq!(
                (price.input, price.cached_input, price.output),
                (10, 5, 20),
                "{base}"
            );
        }
        for price in [
            WorkModelPrice {
                input: 1,
                cached_input: 0,
                output: 0,
            },
            WorkModelPrice {
                input: 0,
                cached_input: 1,
                output: 0,
            },
            WorkModelPrice {
                input: 0,
                cached_input: 0,
                output: 1,
            },
        ] {
            let entry = WorkModelEntry {
                price: Some(price.clone()),
                ..priced.clone()
            };
            let resolved = priced_locally(entry, true);
            assert!(!resolved.is_free_compatible());
            assert_eq!(resolved.price, Some(price));
        }
    }

    #[derive(Default)]
    pub(super) struct MemoryVault(Mutex<HashMap<P, Arc<LeadSecret>>>);
    impl keys::KeyVault for MemoryVault {
        fn load(&self, provider: P) -> Result<Arc<LeadSecret>, keys::LeadKeyError> {
            self.0
                .lock()
                .unwrap()
                .get(&provider)
                .cloned()
                .ok_or(keys::LeadKeyError::Missing)
        }
        fn store(&self, provider: P, secret: LeadSecret) -> Result<(), keys::LeadKeyError> {
            self.0.lock().unwrap().insert(provider, Arc::new(secret));
            Ok(())
        }
        fn clear(&self, provider: P) -> Result<(), keys::LeadKeyError> {
            self.0.lock().unwrap().remove(&provider);
            Ok(())
        }
        fn present(&self, provider: P) -> Result<bool, keys::LeadKeyError> {
            Ok(self.0.lock().unwrap().contains_key(&provider))
        }
    }

    struct Memory(Mutex<HashMap<String, String>>);

    impl WorkModelSettings for Memory {
        fn get(&self, key: &str) -> Option<String> {
            self.0.lock().unwrap().get(key).cloned()
        }
        fn set(&self, key: String, value: String) -> bool {
            self.0.lock().unwrap().insert(key, value);
            true
        }
    }

    struct RejectWrite {
        inner: Arc<dyn WorkModelSettings>,
        key: String,
    }
    impl WorkModelSettings for RejectWrite {
        fn get(&self, key: &str) -> Option<String> {
            self.inner.get(key)
        }
        fn set(&self, key: String, value: String) -> bool {
            key != self.key && self.inner.set(key, value)
        }
    }

    fn with_keys(present: &[WorkModelProvider]) {
        let mut state = state();
        *state = State::default();
        for provider in KEYED_PROVIDERS {
            state.presence.insert(provider, present.contains(&provider));
        }
    }

    #[tokio::test]
    async fn defaults_choices_failures_and_stale_results_stay_truthful() {
        install_settings(Arc::new(Memory(Mutex::new(HashMap::new()))));
        set_compatible_base(Some("https://old-provider.example/v1"))
            .await
            .unwrap();
        let retained = CompatibleCredential {
            generation: generation(P::Compatible),
        };
        set_compatible_base(Some("https://new-provider.example/v1"))
            .await
            .unwrap();
        vault()
            .store(
                P::Compatible,
                LeadSecret::new("replacement-key".into()).unwrap(),
            )
            .unwrap();
        assert_eq!(
            retained.secret().await.err(),
            Some(WorkModelError::Unauthorized),
            "a retained endpoint must not receive its replacement's key"
        );
        let current = CompatibleCredential {
            generation: generation(P::Compatible),
        };
        assert!(current.secret().await.is_ok());
        clear_key(P::Compatible).await.unwrap();
        assert_eq!(
            current.secret().await.err(),
            Some(WorkModelError::Unauthorized)
        );
        let guard = mutations().lock().await;
        let queued = CompatibleCredential {
            generation: generation(P::Compatible),
        };
        let waiting = tokio::spawn(async move { queued.secret().await.err() });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        vault()
            .store(
                P::Compatible,
                LeadSecret::new("rotated-key".into()).unwrap(),
            )
            .unwrap();
        *state().generations.entry(P::Compatible).or_default() += 1;
        drop(guard);
        assert_eq!(waiting.await.unwrap(), Some(WorkModelError::Unauthorized));
        let retained = VaultCredential {
            provider: P::OpenAi,
            generation: generation(P::OpenAi),
        };
        clear_key(P::OpenAi).await.unwrap();
        assert_eq!(
            retained.secret().await.err(),
            Some(WorkModelError::Unauthorized)
        );
        set_compatible_base(None).await.unwrap();
        let profile = ProfileId::from(41_u128);
        with_keys(&[P::OpenAi]);
        apply_check(
            P::OpenAi,
            LeadKeyCheck::Valid(
                models::builtin()
                    .into_iter()
                    .filter(|e| e.model.provider == P::OpenAi)
                    .collect(),
            ),
        );
        fill_defaults(profile, P::OpenAi);
        let ids = |role| effective(&state(), profile, role).map(|entry| entry.id);
        assert_eq!(
            ids(WorkModelRole::Lead).as_deref(),
            Some("openai/gpt-6-luna")
        );
        assert_eq!(
            ids(WorkModelRole::Page).as_deref(),
            Some("openai/gpt-6-luna")
        );
        state().presence.insert(P::Anthropic, true);
        apply_check(
            P::Anthropic,
            LeadKeyCheck::Valid(
                models::builtin()
                    .into_iter()
                    .filter(|e| e.model.provider == P::Anthropic)
                    .collect(),
            ),
        );
        fill_defaults(profile, P::Anthropic);
        assert_eq!(
            ids(WorkModelRole::Lead).as_deref(),
            Some("openai/gpt-6-luna")
        );
        choose(
            profile,
            WorkModelRole::Lead,
            Some("anthropic/claude-sonnet-5-5"),
        )
        .unwrap();
        fill_defaults(profile, P::OpenAi);
        assert_eq!(
            ids(WorkModelRole::Lead).as_deref(),
            Some("anthropic/claude-sonnet-5-5")
        );
        assert!(ready(profile).await);
        for (check, fault) in [
            (LeadKeyCheck::Billing, WorkKeyFault::Billing),
            (LeadKeyCheck::RateLimited, WorkKeyFault::RateLimited),
            (LeadKeyCheck::ProviderDown, WorkKeyFault::ProviderDown),
            (LeadKeyCheck::Unreachable, WorkKeyFault::Offline),
        ] {
            assert_eq!(apply_check(P::Anthropic, check), WorkKeyStatus::Valid);
            assert_eq!(provider_failure(P::Anthropic), Some(fault));
        }
        apply_check(P::Anthropic, LeadKeyCheck::Invalid);
        assert!(!ready(profile).await);
        assert_eq!(
            stored_choice(&state(), profile, WorkModelRole::Lead)
                .unwrap()
                .id,
            "anthropic/claude-sonnet-5-5"
        );
        // An in-flight response using the previous credential cannot invalidate its replacement.
        state().generations.insert(P::Anthropic, 1);
        record_observation(P::Anthropic, 1, Some(WorkKeyStatus::Valid), None);
        record_failure(P::Anthropic, 0, Some(WorkKeyFault::WrongKey));
        assert!(ready(profile).await);
        assert_eq!(provider_failure(P::Anthropic), None);
        assert!(choose(profile, WorkModelRole::Decision, Some("openai/gpt-6-luna")).is_err());
        let other = ProfileId::from(42_u128);
        with_keys(&[P::DeepSeek]);
        assert_eq!(
            effective(&state(), other, WorkModelRole::Page)
                .unwrap()
                .model
                .model,
            "deepseek-flash"
        );
        assert!(ready(other).await);
        with_keys(&[P::OpenRouter]);
        assert!(ready(other).await);
        with_keys(&[]);
        assert!(!ready(other).await);
        // Automatic must use the same local pricing as an explicit choice.
        let local = models::builtin().into_iter().next().unwrap();
        let local = WorkModelEntry {
            id: "compatible/local-model".into(),
            model: WorkModelRef {
                provider: P::Compatible,
                ..local.model
            },
            roles: ROLES.to_vec(),
            price: None,
            ..local
        };
        state().listed.insert(P::Compatible, vec![local]);
        assert!(store_setting(
            COMPATIBLE_BASE_KEY.into(),
            "http://localhost:1234/v1".into()
        ));
        for role in ROLES {
            assert!(stored_choice(&state(), other, role).is_none());
            let entry = effective(&state(), other, role).unwrap();
            let price = entry.price.expect("Automatic local models are priced");
            assert_eq!((price.input, price.cached_input, price.output), (0, 0, 0));
        }
        assert!(store_setting(
            COMPATIBLE_BASE_KEY.into(),
            "https://models.example.com/v1".into()
        ));
        for role in ROLES {
            assert!(effective(&state(), other, role).unwrap().price.is_none());
        }
        // Fresh remote listings are authoritative, including explicit zero.
        state().listed.get_mut(&P::Compatible).unwrap()[0].price = Some(WorkModelPrice {
            input: 0,
            cached_input: 0,
            output: 0,
        });
        for role in ROLES {
            assert!(effective(&state(), other, role)
                .unwrap()
                .is_free_compatible());
        }
        let id = "compatible/local-model";
        choose(other, WorkModelRole::Lead, Some(id)).unwrap();
        // A restart drops session listings and memory choices, not persisted pricing provenance.
        with_keys(&[]);
        assert!(
            stored_choice(&state(), other, WorkModelRole::Lead)
                .unwrap()
                .is_free_compatible(),
            "explicit remote zero prices survive restart"
        );
        // Choices made under a local endpoint must not retain derived zero when it becomes remote.
        assert!(store_setting(
            COMPATIBLE_BASE_KEY.into(),
            "http://localhost:1234/v1".into()
        ));
        let mut local = stored_choice(&state(), other, WorkModelRole::Lead).unwrap();
        local.price = None;
        state().listed.insert(P::Compatible, vec![local]);
        choose(other, WorkModelRole::Page, Some(id)).unwrap();
        with_keys(&[]);
        assert!(store_setting(
            COMPATIBLE_BASE_KEY.into(),
            "https://models.example.com/v1".into()
        ));
        assert!(stored_choice(&state(), other, WorkModelRole::Page)
            .unwrap()
            .price
            .is_none());
        // An old choice has no provenance: retain conservative pricing unless freshly listed.
        let mut legacy = stored_choice(&state(), other, WorkModelRole::Lead).unwrap();
        legacy.price = Some(WorkModelPrice {
            input: 0,
            cached_input: 0,
            output: 0,
        });
        assert!(store_setting(
            choice_key(other, WorkModelRole::Light),
            serde_json::to_string(&legacy).unwrap()
        ));
        assert!(stored_choice(&state(), other, WorkModelRole::Light)
            .unwrap()
            .price
            .is_none());
        state().listed.insert(P::Compatible, vec![legacy]);
        for role in ROLES {
            assert!(
                stored_choice(&state(), other, role)
                    .unwrap()
                    .is_free_compatible(),
                "fresh explicit prices supersede stale or unclassified stored prices"
            );
            choose(other, role, None).unwrap();
        }
        let rate = WorkModelPrice {
            input: 10,
            cached_input: 5,
            output: 20,
        };
        let mut billed = state().listed.get(&P::Compatible).unwrap()[0].clone();
        billed.price = Some(rate.clone());
        state().listed.insert(P::Compatible, vec![billed]);
        assert!(store_setting(
            COMPATIBLE_BASE_KEY.into(),
            "http://100.64.1.2/v1".into()
        ));
        choose(other, WorkModelRole::Lead, Some(id)).unwrap();
        with_keys(&[]);
        assert_eq!(
            stored_choice(&state(), other, WorkModelRole::Lead)
                .unwrap()
                .price,
            Some(rate.clone())
        );
        assert!(store_setting(
            COMPATIBLE_BASE_KEY.into(),
            "https://models.example.com/v1".into()
        ));
        assert!(
            stored_choice(&state(), other, WorkModelRole::Lead)
                .unwrap()
                .price
                .is_none(),
            "a different endpoint must re-list its own explicit rates"
        );
        choose(other, WorkModelRole::Lead, None).unwrap();
        assert!(store_setting(COMPATIBLE_BASE_KEY.into(), String::new()));
        // Legacy explicit rates bind to the unchanged endpoint, including zero;
        // an unopened profile binds to the immutable pre-change origin instead.
        assert!(store_setting(LEGACY_PRICE_ORIGIN_KEY.into(), String::new()));
        assert!(store_setting(
            COMPATIBLE_BASE_KEY.into(),
            "https://legacy-price.example/v1".into()
        ));
        for rate in [
            WorkModelPrice {
                input: 0,
                cached_input: 0,
                output: 0,
            },
            WorkModelPrice {
                input: 11,
                cached_input: 4,
                output: 21,
            },
        ] {
            let row = WorkModelEntry {
                id: "compatible/legacy-price".into(),
                model: WorkModelRef {
                    provider: P::Compatible,
                    ..models::builtin().into_iter().next().unwrap().model
                },
                roles: ROLES.to_vec(),
                price: Some(rate.clone()),
                ..models::builtin().into_iter().next().unwrap()
            };
            let legacy = serde_json::to_string(&StoredChoice {
                entry: row,
                local_price: Some(false),
                pricing_endpoint: None,
            })
            .unwrap();
            assert!(store_setting(
                choice_key(other, WorkModelRole::Lead),
                legacy.clone()
            ));
            assert!(store_setting(
                choice_key(other, WorkModelRole::Page),
                legacy
            ));
            let writable = SETTINGS.read().unwrap().clone().unwrap();
            install_settings(Arc::new(RejectWrite {
                inner: writable.clone(),
                key: choice_key(other, WorkModelRole::Lead),
            }));
            let retained = stored_choice(&state(), other, WorkModelRole::Lead)
                .expect("a choice must remain readable when its setting cannot be written");
            assert_eq!(retained.id, "compatible/legacy-price");
            assert_eq!(retained.price, Some(rate));
            set_compatible_base(Some("https://legacy-price-new.example/v1"))
                .await
                .unwrap();
            assert!(stored_choice(&state(), other, WorkModelRole::Lead)
                .unwrap()
                .price
                .is_none());
            assert!(stored_choice(&state(), other, WorkModelRole::Page)
                .unwrap()
                .price
                .is_none());
            assert_eq!(
                stored_choice(&state(), other, WorkModelRole::Lead)
                    .unwrap()
                    .id,
                "compatible/legacy-price"
            );
            install_settings(writable);
            choose(other, WorkModelRole::Lead, None).unwrap();
            choose(other, WorkModelRole::Page, None).unwrap();
            assert!(store_setting(LEGACY_PRICE_ORIGIN_KEY.into(), String::new()));
            assert!(store_setting(
                COMPATIBLE_BASE_KEY.into(),
                "https://legacy-price.example/v1".into()
            ));
        }
        assert!(store_setting(LEGACY_PRICE_ORIGIN_KEY.into(), String::new()));
        assert!(store_setting(
            COMPATIBLE_BASE_KEY.into(),
            "https://anchor-old.example/v1".into()
        ));
        vault()
            .store(
                P::Compatible,
                LeadSecret::new("anchor-existing-key".into()).unwrap(),
            )
            .unwrap();
        let previous = SETTINGS.read().unwrap().clone().unwrap();
        install_settings(Arc::new(RejectWrite {
            inner: previous.clone(),
            key: LEGACY_PRICE_ORIGIN_KEY.into(),
        }));
        assert_eq!(
            set_compatible_base(Some("https://anchor-new.example/v1"))
                .await
                .err(),
            Some(WorkModelError::Unauthorized)
        );
        assert_eq!(
            compatible_base().as_deref(),
            Some("https://anchor-old.example/v1")
        );
        assert!(
            vault().load(P::Compatible).is_ok(),
            "a failed anchor write cleared the existing credential"
        );
        install_settings(Arc::new(RejectWrite {
            inner: previous.clone(),
            key: COMPATIBLE_BASE_KEY.into(),
        }));
        assert_eq!(
            set_compatible_base(Some("https://anchor-new.example/v1"))
                .await
                .err(),
            Some(WorkModelError::Unauthorized)
        );
        assert_eq!(
            compatible_base().as_deref(),
            Some("https://anchor-old.example/v1")
        );
        assert_eq!(
            serde_json::from_str::<Option<String>>(&setting(LEGACY_PRICE_ORIGIN_KEY).unwrap())
                .unwrap()
                .as_deref(),
            Some("https://anchor-old.example/v1")
        );
        install_settings(previous);
        // Endpoint changes invalidate the previous listing's pricing, including
        // local-to-local changes and a later return to the original endpoint.
        for explicit in [
            None,
            Some(WorkModelPrice {
                input: 0,
                cached_input: 0,
                output: 0,
            }),
            Some(WorkModelPrice {
                input: 10,
                cached_input: 5,
                output: 20,
            }),
        ] {
            set_compatible_base(Some("https://pricing-first.example/v1"))
                .await
                .unwrap();
            let mut row = models::builtin().into_iter().next().unwrap();
            row.id = "compatible/pricing-origin".into();
            row.model.provider = P::Compatible;
            row.roles = ROLES.to_vec();
            row.price = explicit.clone();
            state().listed.insert(P::Compatible, vec![row.clone()]);
            choose(other, WorkModelRole::Lead, Some(&row.id)).unwrap();
            assert_eq!(
                stored_choice(&state(), other, WorkModelRole::Lead)
                    .unwrap()
                    .price,
                explicit
            );
            set_compatible_base(Some("http://localhost:1234/v1"))
                .await
                .unwrap();
            assert!(stored_choice(&state(), other, WorkModelRole::Lead)
                .unwrap()
                .is_free_compatible());
            row.price = explicit.clone();
            state().listed.insert(P::Compatible, vec![row.clone()]);
            choose(other, WorkModelRole::Lead, Some(&row.id)).unwrap();
            with_keys(&[]);
            let same_endpoint = stored_choice(&state(), other, WorkModelRole::Lead).unwrap();
            assert_eq!(
                same_endpoint.price,
                explicit.clone().or(Some(WorkModelPrice {
                    input: 0,
                    cached_input: 0,
                    output: 0
                }))
            );
            set_compatible_base(Some("http://localhost:5678/v1"))
                .await
                .unwrap();
            assert!(
                stored_choice(&state(), other, WorkModelRole::Lead)
                    .unwrap()
                    .is_free_compatible(),
                "a different local endpoint must not inherit an old explicit charge"
            );
            set_compatible_base(Some("https://pricing-second.example/v1"))
                .await
                .unwrap();
            assert!(
                stored_choice(&state(), other, WorkModelRole::Lead)
                    .unwrap()
                    .price
                    .is_none(),
                "local-derived or old-endpoint prices must not reach the remote endpoint"
            );
            set_compatible_base(Some("http://localhost:1234/v1"))
                .await
                .unwrap();
            set_compatible_base(Some("https://pricing-first.example/v1"))
                .await
                .unwrap();
            assert!(stored_choice(&state(), other, WorkModelRole::Lead)
                .unwrap()
                .price
                .is_none());
            row.price = explicit.clone();
            state().listed.insert(P::Compatible, vec![row]);
            assert_eq!(
                stored_choice(&state(), other, WorkModelRole::Lead)
                    .unwrap()
                    .price,
                explicit
            );
            choose(other, WorkModelRole::Lead, None).unwrap();
        }
        // All test secret operations use the injected memory vault.
        vault()
            .store(P::OpenAi, LeadSecret::new("test-key".into()).unwrap())
            .unwrap();
        assert!(vault().present(P::OpenAi).unwrap());
        vault().clear(P::OpenAi).unwrap();
        assert!(!vault().present(P::OpenAi).unwrap());

        // Search follows the chosen models, then an OpenAI key, else none.
        with_keys(&[P::DeepSeek]);
        assert_eq!(
            search_readiness(other).await,
            WorkSearchReadiness::Unavailable
        );
        with_keys(&[P::DeepSeek, P::OpenAi]);
        choose(other, WorkModelRole::Lead, Some("deepseek/deepseek-v4-pro")).unwrap();
        assert_eq!(search_readiness(other).await, WorkSearchReadiness::OpenAi);
        choose(other, WorkModelRole::Lead, None).unwrap();
        with_keys(&[P::Anthropic]);
        assert!(matches!(
            search_readiness(other).await,
            WorkSearchReadiness::Native(entry) if entry.model.provider == P::Anthropic
        ));

        // Signed in with no keys, hosted models carry every role.
        struct Session;
        impl WorkCloudSession for Session {
            fn signed_in(&self) -> bool {
                true
            }
            fn plan(&self) -> Option<String> {
                Some("free".into())
            }
            fn base(&self) -> String {
                "https://api.zephium.app".into()
            }
            fn credential(&self) -> Arc<dyn LeadCredential> {
                Arc::new(LeadStaticCredential::new(
                    LeadSecret::new("session".into()).unwrap(),
                ))
            }
        }
        let hosted = profile_id(43);
        with_keys(&[]);

        // Readers that ask together share one catalog fetch, and a fetch that
        // failed is not repeated on the next read.
        static ASKED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        struct Refused;
        impl LeadCredential for Refused {
            fn secret(&self) -> LeadSecretFuture<'_> {
                ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async { Err(WorkModelError::MissingKey) })
            }
            fn rejected(&self) {}
        }
        struct Failing;
        impl WorkCloudSession for Failing {
            fn signed_in(&self) -> bool {
                true
            }
            fn plan(&self) -> Option<String> {
                None
            }
            fn base(&self) -> String {
                "https://api.zephium.app".into()
            }
            fn credential(&self) -> Arc<dyn LeadCredential> {
                Arc::new(Refused)
            }
        }
        install_cloud_session(Arc::new(Failing));
        cloud_session_changed();
        tokio::join!(
            refresh_cloud_catalog(),
            refresh_cloud_catalog(),
            refresh_cloud_catalog()
        );
        refresh_cloud_catalog().await;
        assert_eq!(ASKED.load(std::sync::atomic::Ordering::SeqCst), 1);

        install_cloud_session(Arc::new(Session));
        state().cloud = models::parse_cloud(&serde_json::json!({"models": [
            {"id": "fast", "provider": "openai", "upstream_model": "gpt-6-luna",
             "display_name": "GPT-6 Luna", "roles": ["lead", "page", "light"], "recommended": true,
             "supports": {"tools": true, "vision": true}},
            {"id": "deep", "provider": "anthropic", "upstream_model": "claude-opus-5-5",
             "display_name": "Claude Opus 5.5", "roles": ["lead"], "recommended": true,
             "supports": {"tools": true}}
        ]}));
        assert!(ready(hosted).await);
        let lead = effective(&state(), hosted, WorkModelRole::Lead).unwrap();
        assert_eq!(lead.model.provider, P::Cloud);
        assert!(client(&state(), &lead).is_ok());
        let view = view(hosted).await;
        assert!(view.cloud_signed_in);
        assert_eq!(view.cloud_plan.as_deref(), Some("free"));
        assert!(view.entries.iter().any(|entry| entry.id == "zephium/deep"));
        choose(hosted, WorkModelRole::Lead, Some("zephium/deep")).unwrap();
        assert_eq!(
            effective(&state(), hosted, WorkModelRole::Lead).unwrap().id,
            "zephium/deep"
        );

        // A plan-locked model stays listed but is never chosen or called, and
        // the server's defaults decide each role; a null one is not guessed.
        let catalog = serde_json::json!({
            "defaults": {"lead": "fast", "page": null, "light": "fast", "decision": null},
            "models": [
                {"id": "fast", "provider": "openai", "upstream_model": "gpt-6-luna",
                 "display_name": "GPT-6 Luna", "roles": ["lead", "page", "light"],
                 "available": true, "supports": {"tools": true}},
                {"id": "deep", "provider": "anthropic", "upstream_model": "claude-opus-5-5",
                 "display_name": "Claude Opus 5.5", "roles": ["lead"], "recommended": true,
                 "available": false, "supports": {"tools": true}}
            ]
        });
        state().cloud = models::parse_cloud(&catalog);
        state().cloud_defaults = models::parse_cloud_defaults(&catalog);
        assert_eq!(effective(&state(), hosted, WorkModelRole::Lead), None);
        let deep = stored_choice(&state(), hosted, WorkModelRole::Lead).unwrap();
        assert_eq!(
            client(&state(), &deep).err(),
            Some(WorkModelError::BadRequest)
        );
        assert_eq!(
            choose(hosted, WorkModelRole::Lead, Some("zephium/deep")),
            Err(WorkModelError::BadRequest)
        );
        choose(hosted, WorkModelRole::Lead, None).unwrap();
        let lead = effective(&state(), hosted, WorkModelRole::Lead).unwrap();
        assert_eq!(lead.id, "zephium/fast");
        choose(hosted, WorkModelRole::Lead, Some("zephium/fast")).unwrap();
        assert_eq!(family_entry(&state(), P::Cloud, WorkModelRole::Page), None);
        assert_eq!(
            super::view(hosted).await.cloud_locked,
            vec!["zephium/deep".to_owned()]
        );
        assert_eq!(
            super::view(hosted).await.cloud_locked_plans,
            vec![("zephium/deep".to_owned(), None)]
        );

        // Signing out takes the hosted catalog with it; the choice stays for later.
        if let Ok(mut slot) = CLOUD.write() {
            *slot = None;
        }
        cloud_session_changed();
        assert!(state().cloud.is_empty());
        assert!(!ready(hosted).await);
        assert!(stored_choice(&state(), hosted, WorkModelRole::Lead).is_some());

        // The lead's effort is the profile's, held to what each model takes.
        let tuned = profile_id(44);
        with_keys(&[P::OpenAi, P::DeepSeek, P::Mistral]);
        choose(tuned, WorkModelRole::Lead, Some("openai/gpt-6-luna")).unwrap();
        let view = super::view(tuned).await;
        assert_eq!(view.effort, Some(WorkModelReasoning::Medium));
        assert!(view
            .efforts
            .iter()
            .any(|(id, levels)| id == "openai/gpt-6-luna"
                && levels.last() == Some(&WorkModelReasoning::Max)));
        assert!(!view
            .efforts
            .iter()
            .any(|(id, _)| id == "mistral/mistral-medium-3-5"));
        set_effort(tuned, Some(WorkModelReasoning::Max)).unwrap();
        assert_eq!(
            super::view(tuned).await.effort,
            Some(WorkModelReasoning::Max)
        );
        let luna = effective(&state(), tuned, WorkModelRole::Lead).unwrap();
        assert_eq!(lead_effort(tuned, &luna), Some(WorkModelReasoning::Max));
        choose(tuned, WorkModelRole::Lead, Some("deepseek/deepseek-v4-pro")).unwrap();
        assert_eq!(
            super::view(tuned).await.effort,
            Some(WorkModelReasoning::High)
        );
        choose(
            tuned,
            WorkModelRole::Lead,
            Some("mistral/mistral-medium-3-5"),
        )
        .unwrap();
        assert_eq!(super::view(tuned).await.effort, None);
        set_effort(tuned, None).unwrap();
        choose(tuned, WorkModelRole::Lead, Some("openai/gpt-6-luna")).unwrap();
        assert_eq!(
            super::view(tuned).await.effort,
            Some(WorkModelReasoning::Medium)
        );
    }

    fn profile_id(id: u128) -> ProfileId {
        ProfileId::from(id)
    }
}
