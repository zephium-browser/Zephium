//! The account as the app sees it: one per install, shared by every profile.
//!
//! State changes only here. Callers read a [`View`] and are told through the
//! observer when it changes. Async results carry the epoch they started in, so
//! a reply that arrives after sign-out or a server change is dropped.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Notify;
use url::Url;

use crate::client::{AvatarReply, Client, ConfigReply, Origin};
use crate::tasks::{CallSource, Ledger};
use crate::vault::{AccountVault, VaultFault};
use crate::wire::{
    Billing, ClientConfig, PlanChange, Profile, Purchase, Receipt, Session, Usage, UsageHistory,
};
use crate::{older_than, AccountError, AccountToken, DEFAULT_ORIGIN};

const SERVER_KEY: &str = "account.server";
const CACHE_KEY: &str = "account.cache";
const CHANGE_KEY: &str = "account.plan_change";
const TASKS_KEY: &str = "account.call_tasks";
const MAX_CACHE_BYTES: usize = 64 * 1024;
const MAX_EMAIL_BYTES: usize = 254;
const MAX_NAME_CHARS: usize = 100;
/// Matches the service's resend cooldown; the server's own answer still wins.
const RESEND_AFTER: Duration = Duration::from_secs(60);
/// Plan and usage older than this are fetched again when someone looks.
const STALE_AFTER: Duration = Duration::from_secs(60);
/// How long a vault that refused is left alone before it is asked again.
const VAULT_PAUSE: Duration = Duration::from_secs(60);
const CHECKOUT_WATCH: Duration = Duration::from_secs(15 * 60);
/// How long a received checkout link is offered again instead of asking the
/// service for another, which it refuses while one is open.
const CHECKOUT_LINK: Duration = Duration::from_secs(30 * 60);
const MAX_DEVICE_CHARS: usize = 64;
/// The service allows six billing refreshes a minute per account.
const BILLING_REFRESHES: usize = 6;
const BILLING_WINDOW: Duration = Duration::from_secs(60);
const MAX_UNSETTLED: usize = 8;
/// A held account picture is revalidated when shown, at most this often.
#[cfg(not(test))]
const AVATAR_RECHECK: Duration = Duration::from_secs(60);
#[cfg(test)]
const AVATAR_RECHECK: Duration = Duration::ZERO;

/// App settings, installed by the composition root.
pub trait AccountSettings: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    /// Stores a value; an empty value clears it.
    fn set(&self, key: String, value: String) -> bool;
}

/// What the person sees. Never the bearer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub server: String,
    pub custom_server: bool,
    pub config: Option<ClientConfig>,
    /// The server asks for a newer Zephium than this one.
    pub update_required: bool,
    pub signed_in: bool,
    /// The address a code was sent to, while waiting for it.
    pub pending_email: Option<String>,
    /// Seconds until another code may be sent.
    pub resend_in: Option<u32>,
    pub profile: Option<Profile>,
    pub billing: Option<Billing>,
    pub usage: Option<Usage>,
    /// A checkout was opened and has not been seen to complete.
    pub checkout_open: bool,
    /// The billing portal was opened and billing is being read until it changes.
    pub portal_open: bool,
    /// Hosted calls whose charge was not confirmed when they ended, as last
    /// read from their receipts.
    pub unsettled: Vec<Receipt>,
    /// The account is under billing review: no purchase or upgrade is taken.
    pub billing_review: bool,
    /// A Pro/Max change this account has started and not dismissed.
    pub plan_change: Option<PlanChange>,
    /// The revision of the account picture held here, to ask
    /// [`Service::avatar_image`] for; none shows initials.
    pub avatar: Option<u32>,
    /// The session is signed in, but the OS vault would not give up its
    /// bearer, so nothing that needs the server can run.
    pub vault_fault: bool,
}

/// The account picture's bytes, for one account at one address on one server.
/// Kept in memory only, so it goes with the session.
struct HeldAvatar {
    origin: String,
    account: String,
    url: String,
    etag: Option<String>,
    bytes: Arc<Vec<u8>>,
    /// Never reused while the app runs, so a picture shown by revision is
    /// never another account's.
    revision: u32,
    checked: Instant,
}

/// A plan change kept across restarts, for one account on one server. It holds
/// ids and prices only.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct HeldChange {
    origin: String,
    account: String,
    change: PlanChange,
}

struct HeldLink {
    purchase: Purchase,
    url: Url,
    at: Instant,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Cache {
    #[serde(default)]
    origin: String,
    profile: Option<Profile>,
    billing: Option<Billing>,
    usage: Option<Usage>,
}

struct Pending {
    email: String,
    resend_at: Instant,
}

struct Inner {
    client: Arc<Client>,
    custom_server: bool,
    epoch: u64,
    signed_in: bool,
    token: Option<Arc<AccountToken>>,
    pending: Option<Pending>,
    config: Option<ClientConfig>,
    config_etag: Option<String>,
    config_until: Option<Instant>,
    profile: Option<Profile>,
    billing: Option<Billing>,
    usage: Option<Usage>,
    refreshed: Option<Instant>,
    checkout_until: Option<Instant>,
    portal_until: Option<Instant>,
    billing_refreshes: VecDeque<Instant>,
    unsettled: Vec<Receipt>,
    billing_review: bool,
    checkout_link: Option<HeldLink>,
    /// Outlives a re-authentication: the quote is confirmed after it.
    change: Option<HeldChange>,
    tasks: Ledger,
    avatar: Option<HeldAvatar>,
    avatar_revisions: u32,
    /// An address the service answered 404 for, until the account names another.
    avatar_missing: Option<String>,
    /// Cancels the upload under way.
    avatar_upload: Option<Arc<Notify>>,
    /// When the vault last refused to give up the bearer. Until the pause
    /// has passed it is not asked again, so a denied Keychain is not
    /// prompted on every call.
    vault_fault: Option<Instant>,
}

impl Inner {
    fn origin(&self) -> String {
        self.client.origin().as_str().to_owned()
    }

    fn clear_account(&mut self) {
        self.token = None;
        self.profile = None;
        self.billing = None;
        self.usage = None;
        self.refreshed = None;
        self.checkout_until = None;
        self.portal_until = None;
        self.billing_refreshes.clear();
        self.unsettled.clear();
        self.billing_review = false;
        self.checkout_link = None;
        self.avatar = None;
        self.avatar_missing = None;
        self.vault_fault = None;
        if let Some(cancel) = self.avatar_upload.take() {
            cancel.notify_one();
        }
    }

    /// The picture held, when it is the signed-in account's current one here.
    fn held_avatar(&self) -> Option<&HeldAvatar> {
        let profile = self.profile.as_ref().filter(|_| self.signed_in)?;
        let url = profile.avatar_url.as_deref()?;
        let origin = self.origin();
        self.avatar
            .as_ref()
            .filter(|held| held.origin == origin && held.account == profile.id && held.url == url)
    }

    /// The held change, when it belongs to the account signed in on this server.
    fn visible_change(&self) -> Option<&PlanChange> {
        let held = self.change.as_ref()?;
        let account = self.profile.as_ref()?;
        (held.origin == self.origin() && held.account == account.id).then_some(&held.change)
    }

    /// Billing review is a fact of the last usage answer, kept until another says otherwise.
    fn note_usage(&mut self, usage: &Result<Usage, AccountError>) {
        match usage {
            Ok(_) => self.billing_review = false,
            Err(AccountError::BillingReview) => self.billing_review = true,
            Err(_) => {}
        }
    }
}

type Observer = Box<dyn Fn() + Send + Sync>;

pub struct Service {
    settings: Arc<dyn AccountSettings>,
    vault: Arc<dyn AccountVault>,
    device: String,
    version: String,
    inner: Mutex<Inner>,
    observer: OnceLock<Observer>,
    /// Held across a picture upload, so a removal waits for a cancelled
    /// upload to stop before it is sent.
    avatar_writes: tokio::sync::Mutex<()>,
}

impl Service {
    /// Reads the configured server and the last known account from settings.
    /// Signed in means a bearer for that server is in the vault; it is not
    /// read until needed. `version` is this build's, for the server's minimum.
    pub fn new(
        settings: Arc<dyn AccountSettings>,
        vault: Arc<dyn AccountVault>,
        device: &str,
        version: &str,
    ) -> Result<Self, AccountError> {
        let custom = settings
            .get(SERVER_KEY)
            .filter(|value| !value.is_empty())
            .and_then(|value| Origin::parse(&value).ok());
        let custom_server = custom.is_some();
        let origin = match custom {
            Some(origin) => origin,
            None => Origin::parse(DEFAULT_ORIGIN)?,
        };
        let signed_in = matches!(vault.present(origin.as_str()), Ok(true));
        let origin_text = origin.as_str().to_owned();
        let cache: Cache = settings
            .get(CACHE_KEY)
            .filter(|_| signed_in)
            .and_then(|text| serde_json::from_str::<Cache>(&text).ok())
            .filter(|cache| cache.origin == origin.as_str())
            .unwrap_or_default();
        let device: String = device
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_DEVICE_CHARS)
            .collect();
        let change = settings
            .get(CHANGE_KEY)
            .and_then(|text| serde_json::from_str::<HeldChange>(&text).ok())
            .filter(|held| held.origin == origin_text);
        let tasks = Ledger::load(settings.get(TASKS_KEY).as_deref());
        Ok(Self {
            settings,
            vault,
            device,
            version: version.to_owned(),
            inner: Mutex::new(Inner {
                client: Arc::new(Client::new(origin)?),
                custom_server,
                epoch: 0,
                signed_in,
                token: None,
                pending: None,
                config: None,
                config_etag: None,
                config_until: None,
                profile: cache.profile,
                billing: cache.billing,
                usage: cache.usage,
                refreshed: None,
                checkout_until: None,
                portal_until: None,
                billing_refreshes: VecDeque::new(),
                unsettled: Vec::new(),
                billing_review: false,
                checkout_link: None,
                change,
                tasks,
                avatar: None,
                avatar_revisions: 0,
                avatar_missing: None,
                avatar_upload: None,
                vault_fault: None,
            }),
            observer: OnceLock::new(),
            avatar_writes: tokio::sync::Mutex::new(()),
        })
    }

    /// Called after every change a view would show.
    pub fn set_observer(&self, observer: Observer) {
        let _ = self.observer.set(observer);
    }

    fn notify(&self) {
        if let Some(observer) = self.observer.get() {
            observer();
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn view(&self) -> View {
        let inner = self.lock();
        let now = Instant::now();
        View {
            server: inner.client.origin().as_str().to_owned(),
            custom_server: inner.custom_server,
            config: inner.config.clone(),
            update_required: inner
                .config
                .as_ref()
                .and_then(|config| config.min_client_version.as_deref())
                .is_some_and(|minimum| older_than(&self.version, minimum)),
            signed_in: inner.signed_in,
            pending_email: inner.pending.as_ref().map(|p| p.email.clone()),
            resend_in: inner
                .pending
                .as_ref()
                .map(|p| p.resend_at.saturating_duration_since(now).as_secs() as u32)
                .filter(|secs| *secs > 0),
            profile: inner.profile.clone(),
            billing: inner.billing.clone(),
            usage: inner.usage.clone(),
            checkout_open: inner.checkout_until.is_some_and(|until| until > now),
            portal_open: inner.portal_until.is_some_and(|until| until > now),
            unsettled: inner.unsettled.clone(),
            billing_review: inner.billing_review,
            plan_change: inner.visible_change().cloned(),
            avatar: inner.held_avatar().map(|held| held.revision),
            vault_fault: inner.signed_in && inner.vault_fault.is_some(),
        }
    }

    pub fn signed_in(&self) -> bool {
        self.lock().signed_in
    }

    pub fn origin(&self) -> String {
        self.lock().origin()
    }

    /// The plan the server reports for the signed-in account.
    pub fn plan(&self) -> Option<String> {
        let inner = self.lock();
        if !inner.signed_in {
            return None;
        }
        inner.usage.as_ref().and_then(|usage| usage.plan.clone())
    }

    fn client(&self) -> (Arc<Client>, u64) {
        let inner = self.lock();
        (inner.client.clone(), inner.epoch)
    }

    /// What the server offers, read before sign-in. A copy younger than the
    /// server's max-age (five minutes at most) is reused; an older one is
    /// revalidated by its ETag. A failure leaves the last answer.
    pub async fn load_config(&self) -> Result<ClientConfig, AccountError> {
        let (client, epoch, etag) = {
            let inner = self.lock();
            if let (Some(config), Some(until)) = (&inner.config, inner.config_until) {
                if Instant::now() < until {
                    return Ok(config.clone());
                }
            }
            let etag = inner.config.as_ref().and(inner.config_etag.clone());
            (inner.client.clone(), inner.epoch, etag)
        };
        let reply = client.client_config(etag.as_deref()).await?;
        let config = {
            let mut inner = self.lock();
            if inner.epoch != epoch {
                return Err(AccountError::Invalid);
            }
            match reply {
                ConfigReply::Fresh {
                    config,
                    etag,
                    max_age,
                } => {
                    inner.config = Some(config.clone());
                    inner.config_etag = etag;
                    inner.config_until = Some(Instant::now() + max_age);
                    config
                }
                ConfigReply::Unchanged { max_age } => {
                    inner.config_until = Some(Instant::now() + max_age);
                    inner.config.clone().ok_or(AccountError::Protocol)?
                }
            }
        };
        self.notify();
        Ok(config)
    }

    /// Sends a sign-in code. The first code an address receives creates its account.
    pub async fn send_code(&self, email: &str) -> Result<(), AccountError> {
        let email = email.trim().to_owned();
        if !plausible_email(&email) {
            return Err(AccountError::Invalid);
        }
        {
            let inner = self.lock();
            if let Some(pending) = &inner.pending {
                let wait = pending.resend_at.saturating_duration_since(Instant::now());
                if pending.email == email && !wait.is_zero() {
                    return Err(AccountError::RateLimited {
                        retry_after: wait.as_secs().max(1) as u32,
                    });
                }
            }
        }
        let (client, epoch) = self.client();
        let sent = client.send_code(&email).await;
        let resend_at = match sent {
            Ok(()) => Instant::now() + RESEND_AFTER,
            Err(AccountError::RateLimited { retry_after }) => {
                Instant::now() + Duration::from_secs(retry_after.into())
            }
            Err(error) => return Err(error),
        };
        {
            let mut inner = self.lock();
            if inner.epoch == epoch {
                inner.pending = Some(Pending { email, resend_at });
            }
        }
        self.notify();
        sent
    }

    /// Sends a sign-in code to the signed-in address, for a change that needs
    /// a fresh session.
    pub async fn send_reauth_code(&self) -> Result<(), AccountError> {
        let email = {
            let inner = self.lock();
            if !inner.signed_in {
                return Err(AccountError::SignedOut);
            }
            inner.profile.as_ref().map(|profile| profile.email.clone())
        }
        .ok_or(AccountError::Invalid)?;
        self.send_code(&email).await
    }

    /// Forgets the address a code was sent to.
    pub fn cancel_code(&self) {
        self.lock().pending = None;
        self.notify();
    }

    /// Signs in with the code sent to the pending address. A session this
    /// install already had on the server is replaced and revoked.
    pub async fn verify(&self, code: &str) -> Result<(), AccountError> {
        let code = code.trim();
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AccountError::InvalidCode);
        }
        let previous = self.bearer().ok();
        let (client, epoch, email, origin) = {
            let inner = self.lock();
            let email = inner
                .pending
                .as_ref()
                .map(|p| p.email.clone())
                .ok_or(AccountError::Invalid)?;
            (inner.client.clone(), inner.epoch, email, inner.origin())
        };
        let token = client.verify_code(&email, code, &self.device).await?;
        if self.lock().epoch != epoch {
            return Err(AccountError::Invalid);
        }
        self.vault
            .store(&origin, &token)
            .map_err(|_| AccountError::Vault)?;
        let token = Arc::new(token);
        {
            let mut inner = self.lock();
            if inner.epoch != epoch {
                drop(inner);
                let _ = self.vault.clear(&origin);
                return Err(AccountError::Invalid);
            }
            inner.epoch += 1;
            inner.clear_account();
            inner.signed_in = true;
            inner.token = Some(token);
            inner.pending = None;
        }
        self.notify();
        if let Some(previous) = previous {
            let _ = client.sign_out(&previous).await;
        }
        let _ = self.refresh(true).await;
        Ok(())
    }

    /// The bearer for this session, read from the vault once.
    pub fn bearer(&self) -> Result<Arc<AccountToken>, AccountError> {
        let (epoch, origin) = {
            let inner = self.lock();
            if !inner.signed_in {
                return Err(AccountError::SignedOut);
            }
            if let Some(token) = &inner.token {
                return Ok(token.clone());
            }
            if inner
                .vault_fault
                .is_some_and(|at| at.elapsed() < VAULT_PAUSE)
            {
                return Err(AccountError::Vault);
            }
            (inner.epoch, inner.origin())
        };
        let token = match self.vault.load(&origin) {
            Ok(token) => Arc::new(token),
            Err(VaultFault::Missing) => {
                self.forget(epoch);
                return Err(AccountError::SignedOut);
            }
            Err(VaultFault::Inaccessible) => {
                let first = {
                    let mut inner = self.lock();
                    let first = inner.epoch == epoch && inner.vault_fault.is_none();
                    if inner.epoch == epoch {
                        inner.vault_fault = Some(Instant::now());
                    }
                    first
                };
                if first {
                    self.notify();
                }
                return Err(AccountError::Vault);
            }
        };
        let mut inner = self.lock();
        if inner.epoch != epoch || !inner.signed_in {
            return Err(AccountError::SignedOut);
        }
        let recovered = inner.vault_fault.take().is_some();
        let token = inner.token.get_or_insert(token).clone();
        drop(inner);
        if recovered {
            self.notify();
        }
        Ok(token)
    }

    /// The hosted gateway refused the bearer: the session is over.
    pub fn bearer_refused(&self) {
        let epoch = self.lock().epoch;
        self.forget(epoch);
    }

    /// A hosted call ran: what was used changed, so the next look asks again.
    pub fn usage_changed(&self) {
        self.lock().refreshed = None;
    }

    /// Fetches profile, plan and usage; skipped while fresh unless forced.
    pub async fn refresh(&self, force: bool) -> Result<(), AccountError> {
        {
            let inner = self.lock();
            if !inner.signed_in {
                return Ok(());
            }
            if !force && inner.refreshed.is_some_and(|at| at.elapsed() < STALE_AFTER) {
                return Ok(());
            }
        }
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        let (profile, billing, usage) = tokio::join!(
            client.profile(&token),
            client.billing(&token),
            client.usage(&token),
        );
        if [
            profile.as_ref().err(),
            billing.as_ref().err(),
            usage.as_ref().err(),
        ]
        .contains(&Some(&AccountError::SignedOut))
        {
            self.forget(epoch);
            return Err(AccountError::SignedOut);
        }
        let profile = profile?;
        {
            let mut inner = self.lock();
            if inner.epoch != epoch {
                return Ok(());
            }
            inner.profile = Some(profile);
            inner.note_usage(&usage);
            // A server with billing or hosted models turned off answers those
            // with an error; the account itself still works.
            inner.billing = billing.ok().or(inner.billing.take());
            inner.usage = usage.ok().or(inner.usage.take());
            inner.refreshed = Some(Instant::now());
            self.persist(&inner);
        }
        self.notify();
        // A picture that cannot be read leaves initials; the account still refreshed.
        let _ = self.load_avatar().await;
        Ok(())
    }

    /// The account picture's bytes for `revision`, while it is still the
    /// signed-in account's current picture on this server.
    pub fn avatar_image(&self, revision: u32) -> Option<Arc<Vec<u8>>> {
        self.lock()
            .held_avatar()
            .filter(|held| held.revision == revision)
            .map(|held| held.bytes.clone())
    }

    /// Fetches the account picture the account names, or revalidates the
    /// bytes held by their ETag. An address the service no longer has makes
    /// the account be read again, and a new one is followed once; otherwise
    /// the account shows initials.
    pub async fn load_avatar(&self) -> Result<(), AccountError> {
        for _ in 0..2 {
            let (client, epoch, url, etag) = {
                let mut inner = self.lock();
                let Some(profile) = inner.profile.as_ref().filter(|_| inner.signed_in) else {
                    return Ok(());
                };
                let Some(url) = profile.avatar_url.clone() else {
                    let dropped = inner.avatar.take().is_some();
                    drop(inner);
                    if dropped {
                        self.notify();
                    }
                    return Ok(());
                };
                if inner.avatar_missing.as_deref() == Some(url.as_str()) {
                    return Ok(());
                }
                let held = inner.held_avatar();
                if held.is_some_and(|held| held.checked.elapsed() < AVATAR_RECHECK) {
                    return Ok(());
                }
                let etag = held.and_then(|held| held.etag.clone());
                (inner.client.clone(), inner.epoch, url, etag)
            };
            let token = self.bearer()?;
            match client.avatar(&token, &url, etag.as_deref()).await {
                Ok(reply) => {
                    self.keep_avatar(epoch, &url, reply);
                    return Ok(());
                }
                Err(AccountError::NotFound) => {
                    let profile = self.signed_out_on_401(epoch, client.profile(&token).await)?;
                    {
                        let mut inner = self.lock();
                        if inner.epoch != epoch {
                            return Ok(());
                        }
                        inner.avatar = None;
                        if profile.avatar_url.as_deref() == Some(url.as_str()) {
                            inner.avatar_missing = Some(url);
                        }
                        inner.profile = Some(profile);
                        self.persist(&inner);
                    }
                    self.notify();
                }
                Err(error) => return self.signed_out_on_401(epoch, Err(error)),
            }
        }
        Ok(())
    }

    fn keep_avatar(&self, epoch: u64, url: &str, reply: AvatarReply) {
        {
            let mut inner = self.lock();
            let account = inner
                .profile
                .as_ref()
                .filter(|profile| profile.avatar_url.as_deref() == Some(url))
                .map(|profile| profile.id.clone());
            let Some(account) = account.filter(|_| inner.epoch == epoch && inner.signed_in) else {
                return;
            };
            match reply {
                AvatarReply::Unchanged => {
                    if inner.held_avatar().is_some() {
                        if let Some(held) = inner.avatar.as_mut() {
                            held.checked = Instant::now();
                        }
                    }
                    return;
                }
                AvatarReply::Fresh { bytes, etag } => {
                    inner.avatar_revisions = inner.avatar_revisions.wrapping_add(1).max(1);
                    let origin = inner.origin();
                    inner.avatar = Some(HeldAvatar {
                        origin,
                        account,
                        url: url.to_owned(),
                        etag,
                        bytes: Arc::new(bytes),
                        revision: inner.avatar_revisions,
                        checked: Instant::now(),
                    });
                }
            }
        }
        self.notify();
    }

    /// Replaces the account picture with `bytes`, a still PNG, JPEG or WebP of
    /// at most 2 MiB, then fetches the service's own copy of it. A removal or
    /// another upload started meanwhile cancels this one.
    pub async fn upload_avatar(&self, bytes: &[u8]) -> Result<(), AccountError> {
        crate::client::avatar_upload_type(bytes)?;
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        let cancel = Arc::new(Notify::new());
        if let Some(previous) = self.lock().avatar_upload.replace(cancel.clone()) {
            previous.notify_one();
        }
        let result = {
            let _writing = self.avatar_writes.lock().await;
            tokio::select! {
                result = client.upload_avatar(&token, bytes) => result,
                () = cancel.notified() => Err(AccountError::AvatarCancelled),
            }
        };
        {
            let mut inner = self.lock();
            if inner
                .avatar_upload
                .as_ref()
                .is_some_and(|held| Arc::ptr_eq(held, &cancel))
            {
                inner.avatar_upload = None;
            }
        }
        let url = self.signed_out_on_401(epoch, result)?;
        {
            let mut inner = self.lock();
            if inner.epoch != epoch {
                return Ok(());
            }
            if let Some(profile) = inner.profile.as_mut() {
                profile.avatar_url = Some(url);
            }
            // The replaced picture's bytes go now; the new ones are fetched.
            inner.avatar = None;
            inner.avatar_missing = None;
            self.persist(&inner);
        }
        self.notify();
        let _ = self.load_avatar().await;
        Ok(())
    }

    /// Stops an upload under way; nothing changes on the service.
    pub fn cancel_avatar_upload(&self) {
        if let Some(cancel) = self.lock().avatar_upload.take() {
            cancel.notify_one();
        }
    }

    /// Removes the account picture. An upload under way is cancelled, and
    /// has stopped, before the removal is sent, so it cannot land after it.
    pub async fn remove_avatar(&self) -> Result<(), AccountError> {
        self.cancel_avatar_upload();
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        let removed = {
            let _writing = self.avatar_writes.lock().await;
            client.remove_avatar(&token).await
        };
        self.signed_out_on_401(epoch, removed)?;
        {
            let mut inner = self.lock();
            if inner.epoch != epoch {
                return Ok(());
            }
            if let Some(profile) = inner.profile.as_mut() {
                profile.avatar_url = None;
            }
            inner.avatar = None;
            inner.avatar_missing = None;
            self.persist(&inner);
        }
        self.notify();
        Ok(())
    }

    /// Changes the name shown for the account; `None` or blank clears it.
    pub async fn set_name(&self, name: Option<&str>) -> Result<(), AccountError> {
        let name = name.map(str::trim).filter(|name| !name.is_empty());
        if name.is_some_and(|name| {
            name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control)
        }) {
            return Err(AccountError::Invalid);
        }
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        let saved = self.signed_out_on_401(epoch, client.set_name(&token, name).await)?;
        {
            let mut inner = self.lock();
            if inner.epoch != epoch {
                return Ok(());
            }
            if let Some(profile) = inner.profile.as_mut() {
                profile.name = saved;
            }
            self.persist(&inner);
        }
        self.notify();
        Ok(())
    }

    /// Sends a code to `email`, the address the account is moving to. The
    /// service asks for a session from the last ten minutes first.
    pub async fn send_email_change_code(&self, email: &str) -> Result<(), AccountError> {
        let email = email.trim();
        let current = self.lock().profile.as_ref().map(|p| p.email.clone());
        if !plausible_email(email)
            || current.is_some_and(|current| current.eq_ignore_ascii_case(email))
        {
            return Err(AccountError::Invalid);
        }
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        self.signed_out_on_401(epoch, client.send_email_change_code(&token, email).await)
    }

    /// Moves the account to `email`. The service ends every session when it
    /// does, so this install signs in again afterwards.
    pub async fn change_email(&self, email: &str, code: &str) -> Result<(), AccountError> {
        let email = email.trim();
        let code = code.trim();
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AccountError::InvalidCode);
        }
        if !plausible_email(email) {
            return Err(AccountError::Invalid);
        }
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        self.signed_out_on_401(epoch, client.change_email(&token, email, code).await)?;
        self.forget(epoch);
        Ok(())
    }

    /// The account's export as the server states it; no credentials in it.
    pub async fn export(&self) -> Result<Value, AccountError> {
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        self.signed_out_on_401(epoch, client.export(&token).await)
    }

    /// One page of hosted request history.
    pub async fn usage_history(&self, cursor: Option<&str>) -> Result<UsageHistory, AccountError> {
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        self.signed_out_on_401(epoch, client.usage_history(&token, cursor).await)
    }

    /// One hosted request's accounting.
    pub async fn receipt(&self, id: &str) -> Result<Receipt, AccountError> {
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        let receipt = self.signed_out_on_401(epoch, client.receipt(&token, id).await)?;
        let changed = {
            let mut inner = self.lock();
            let known =
                inner.epoch == epoch && inner.unsettled.iter().any(|held| held.id == receipt.id);
            if known {
                for held in &mut inner.unsettled {
                    if held.id == receipt.id {
                        *held = receipt.clone();
                    }
                }
            }
            known
        };
        if changed {
            self.notify();
        }
        Ok(receipt)
    }

    /// Reads the receipt of a hosted call whose charge was not confirmed when
    /// it ended, and keeps it for the account page until it settles.
    pub async fn confirm_request(&self, id: &str) -> Result<Receipt, AccountError> {
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        let receipt = self.signed_out_on_401(epoch, client.receipt(&token, id).await)?;
        {
            let mut inner = self.lock();
            if inner.epoch != epoch {
                return Ok(receipt);
            }
            inner.unsettled.retain(|held| held.id != receipt.id);
            inner.unsettled.insert(0, receipt.clone());
            inner.unsettled.truncate(MAX_UNSETTLED);
            inner.refreshed = None;
        }
        self.notify();
        Ok(receipt)
    }

    /// Asks the server to check billing now, after a checkout or portal visit.
    /// Held to the service's limit of six a minute.
    pub async fn refresh_billing(&self) -> Result<Billing, AccountError> {
        let token = self.bearer()?;
        {
            let mut inner = self.lock();
            let now = Instant::now();
            while inner
                .billing_refreshes
                .front()
                .is_some_and(|at| now.duration_since(*at) >= BILLING_WINDOW)
            {
                inner.billing_refreshes.pop_front();
            }
            if inner.billing_refreshes.len() >= BILLING_REFRESHES {
                let oldest = inner.billing_refreshes.front().copied().unwrap_or(now);
                let wait = BILLING_WINDOW.saturating_sub(now.duration_since(oldest));
                return Err(AccountError::RateLimited {
                    retry_after: wait.as_secs().max(1) as u32,
                });
            }
            inner.billing_refreshes.push_back(now);
        }
        let (client, epoch) = self.client();
        let billing = match client.refresh_billing(&token).await {
            Err(AccountError::Invalid | AccountError::NotFound) => client.billing(&token).await,
            other => other,
        };
        let billing = match billing {
            Err(AccountError::SignedOut) => {
                self.forget(epoch);
                return Err(AccountError::SignedOut);
            }
            other => other?,
        };
        let usage = client.usage(&token).await;
        {
            let mut inner = self.lock();
            if inner.epoch == epoch {
                let before = inner.billing.clone();
                inner.billing = Some(billing.clone());
                inner.note_usage(&usage);
                if let Ok(usage) = usage {
                    inner.usage = Some(usage);
                }
                if before.is_some_and(|before| before != billing) {
                    inner.checkout_until = None;
                    inner.portal_until = None;
                    inner.checkout_link = None;
                }
                self.persist(&inner);
            }
        }
        self.notify();
        Ok(billing)
    }

    /// A checkout link on the billing provider, to open in a browser tab. The
    /// link received for a purchase is offered again while it is fresh: the
    /// service refuses a second subscription checkout while one is open.
    pub async fn checkout(&self, purchase: &Purchase) -> Result<Url, AccountError> {
        if let Some(url) = self.held_link(purchase) {
            self.lock().checkout_until = Some(Instant::now() + CHECKOUT_WATCH);
            self.notify();
            return Ok(url);
        }
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        let result = client.checkout(&token, purchase).await;
        self.note_review(&result);
        let url = self.signed_out_on_401(epoch, result)?;
        {
            let mut inner = self.lock();
            inner.checkout_until = Some(Instant::now() + CHECKOUT_WATCH);
            inner.checkout_link = Some(HeldLink {
                purchase: purchase.clone(),
                url: url.clone(),
                at: Instant::now(),
            });
        }
        self.notify();
        Ok(url)
    }

    fn held_link(&self, purchase: &Purchase) -> Option<Url> {
        let inner = self.lock();
        inner
            .checkout_link
            .as_ref()
            .filter(|held| held.purchase == *purchase && held.at.elapsed() < CHECKOUT_LINK)
            .map(|held| held.url.clone())
    }

    /// The billing provider's portal for managing the subscription. Billing is
    /// read for a while afterwards, as after a checkout, so a change made
    /// there shows up here.
    pub async fn portal(&self) -> Result<Url, AccountError> {
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        let url = self.signed_out_on_401(epoch, client.portal(&token).await)?;
        self.lock().portal_until = Some(Instant::now() + CHECKOUT_WATCH);
        self.notify();
        Ok(url)
    }

    /// Whether an opened checkout or portal is still being watched for.
    pub fn checkout_open(&self) -> bool {
        let inner = self.lock();
        let now = Instant::now();
        [inner.checkout_until, inner.portal_until]
            .into_iter()
            .flatten()
            .any(|until| until > now)
    }

    /// Sets the daily spending limit in units, or removes it with `None`.
    pub async fn set_daily_limit(&self, units: Option<&str>) -> Result<(), AccountError> {
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        self.signed_out_on_401(epoch, client.set_daily_limit(&token, units).await)?;
        self.refresh(true).await
    }

    /// Asks for a quote to move to `plan` on `interval`. Nothing is charged.
    pub async fn preview_plan_change(
        &self,
        plan: &str,
        interval: &str,
    ) -> Result<PlanChange, AccountError> {
        let (account, origin) = {
            let inner = self.lock();
            if inner.visible_change().is_some_and(PlanChange::in_flight) {
                return Err(AccountError::PlanChangeInProgress);
            }
            let account = inner
                .profile
                .as_ref()
                .map(|profile| profile.id.clone())
                .ok_or(AccountError::Invalid)?;
            (account, inner.origin())
        };
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        let result = client.preview_plan_change(&token, plan, interval).await;
        self.note_review(&result);
        let change = self.signed_out_on_401(epoch, result)?;
        self.hold_change(epoch, origin, account, &change);
        Ok(change)
    }

    /// Confirms the quote held. A session older than ten minutes answers
    /// `ReauthRequired`; the quote stays, and confirming again after signing
    /// in resumes the same operation, as does a retry after a lost answer.
    pub async fn confirm_plan_change(&self) -> Result<PlanChange, AccountError> {
        let (held, account, origin) = {
            let inner = self.lock();
            let held = inner
                .visible_change()
                .filter(|change| change.status == "quoted" || change.in_flight())
                .cloned()
                .ok_or(AccountError::NotFound)?;
            let account = inner.profile.as_ref().map(|p| p.id.clone());
            (held, account.ok_or(AccountError::Invalid)?, inner.origin())
        };
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        let result = client.confirm_plan_change(&token, &held.id).await;
        self.note_review(&result);
        if result.as_ref().err() == Some(&AccountError::PlanChangeExpired) {
            self.release_change(epoch);
        }
        let change = self.signed_out_on_401(epoch, result)?;
        self.hold_change(epoch, origin, account, &change);
        Ok(change)
    }

    /// Reads where the held change stands. When it has just finished, billing
    /// and usage are read again so the page shows the plan it ended on.
    pub async fn refresh_plan_change(&self) -> Result<Option<PlanChange>, AccountError> {
        let (held, account, origin) = {
            let inner = self.lock();
            let Some(held) = inner.visible_change().cloned() else {
                return Ok(None);
            };
            let account = inner.profile.as_ref().map(|p| p.id.clone());
            (held, account.ok_or(AccountError::Invalid)?, inner.origin())
        };
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        let change = match client.plan_change(&token, &held.id).await {
            Err(AccountError::NotFound) => {
                self.release_change(epoch);
                return Ok(None);
            }
            other => self.signed_out_on_401(epoch, other)?,
        };
        self.hold_change(epoch, origin, account, &change);
        if change.finished() && !held.finished() {
            // The limiter may refuse; the periodic read catches up.
            if self.refresh_billing().await.is_err() {
                self.usage_changed();
            }
        }
        Ok(Some(change))
    }

    /// Puts away a quote, or a change that has finished. One still being
    /// carried out stays: it cannot be taken back.
    pub fn dismiss_plan_change(&self) {
        let epoch = {
            let inner = self.lock();
            if inner.visible_change().is_some_and(PlanChange::in_flight) {
                return;
            }
            inner.epoch
        };
        self.release_change(epoch);
    }

    /// Whether a held change is being carried out.
    pub fn plan_change_in_flight(&self) -> bool {
        self.lock()
            .visible_change()
            .is_some_and(PlanChange::in_flight)
    }

    /// Whether a held change is awaiting a read, such as after a restart.
    pub fn plan_change_open(&self) -> bool {
        self.lock()
            .visible_change()
            .is_some_and(|change| change.in_flight() || change.status == "quoted")
    }

    fn note_review<T>(&self, result: &Result<T, AccountError>) {
        if result.as_ref().err() == Some(&AccountError::BillingReview) {
            self.lock().billing_review = true;
            self.notify();
        }
    }

    fn hold_change(&self, epoch: u64, origin: String, account: String, change: &PlanChange) {
        {
            let mut inner = self.lock();
            if inner.epoch != epoch {
                return;
            }
            let held = HeldChange {
                origin,
                account,
                change: change.clone(),
            };
            if let Ok(text) = serde_json::to_string(&held) {
                self.settings.set(CHANGE_KEY.to_owned(), text);
            }
            inner.change = Some(held);
        }
        self.notify();
    }

    fn release_change(&self, epoch: u64) {
        {
            let mut inner = self.lock();
            if inner.epoch != epoch {
                return;
            }
            inner.change = None;
            self.settings.set(CHANGE_KEY.to_owned(), String::new());
        }
        self.notify();
    }

    /// Notes what a hosted call was made for, so history can group by it.
    /// Stays on this computer.
    pub fn record_call_source(&self, request_id: &str, source: CallSource) {
        let mut inner = self.lock();
        if inner.tasks.record(request_id, source) {
            self.settings.set(TASKS_KEY.to_owned(), inner.tasks.text());
        }
    }

    /// What each of these calls was made for, when known.
    pub fn call_sources(&self, request_ids: &[String]) -> Vec<Option<CallSource>> {
        self.lock().tasks.lookup(request_ids)
    }

    pub async fn sessions(&self) -> Result<Vec<Session>, AccountError> {
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        self.signed_out_on_401(epoch, client.sessions(&token).await)
    }

    pub async fn revoke_session(&self, id: &str) -> Result<(), AccountError> {
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        self.signed_out_on_401(epoch, client.revoke_session(&token, id).await)
    }

    /// Signs out here. The server is told when it can be reached; the vault
    /// entry goes either way.
    pub async fn sign_out(&self) -> Result<(), AccountError> {
        let (client, epoch) = self.client();
        let origin = self.origin();
        // A vault that just refused is not asked again to tell the server.
        if self.lock().vault_fault.is_none() {
            if let Ok(token) = self.bearer() {
                let _ = client.sign_out(&token).await;
            }
        }
        self.forget(epoch);
        self.forget_local_history();
        if self.vault.clear(&origin).is_err() {
            return Err(AccountError::Vault);
        }
        Ok(())
    }

    /// What this computer kept about the account's use: which task made which call.
    fn forget_local_history(&self) {
        let mut inner = self.lock();
        if inner.tasks.clear() {
            self.settings.set(TASKS_KEY.to_owned(), String::new());
        }
    }

    /// Ends every session of the account, this one included. Unlike signing
    /// out here, nothing changes unless the server confirms.
    pub async fn sign_out_everywhere(&self) -> Result<(), AccountError> {
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        match client.sign_out_everywhere(&token).await {
            Ok(()) | Err(AccountError::SignedOut) => {
                self.forget(epoch);
                self.forget_local_history();
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub async fn send_deletion_code(&self) -> Result<(), AccountError> {
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        self.signed_out_on_401(epoch, client.send_deletion_code(&token).await)
    }

    /// Deletes the account on the server, then everything about it here.
    pub async fn delete_account(&self, code: &str) -> Result<(), AccountError> {
        let code = code.trim();
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AccountError::InvalidCode);
        }
        let token = self.bearer()?;
        let (client, epoch) = self.client();
        self.signed_out_on_401(epoch, client.delete_account(&token, code).await)?;
        self.forget(epoch);
        self.forget_local_history();
        let epoch = self.lock().epoch;
        self.release_change(epoch);
        Ok(())
    }

    /// Points the app at another account server, or back at Zephium's with
    /// `None`. Each server keeps its own session in the vault, so switching
    /// back finds it again; nothing is sent to either server.
    pub async fn set_server(&self, server: Option<&str>) -> Result<(), AccountError> {
        let origin = match server.map(str::trim).filter(|s| !s.is_empty()) {
            Some(text) => Origin::parse(text)?,
            None => Origin::parse(DEFAULT_ORIGIN)?,
        };
        let custom = origin.as_str() != DEFAULT_ORIGIN;
        let signed_in = matches!(self.vault.present(origin.as_str()), Ok(true));
        let client = Arc::new(Client::new(origin.clone())?);
        let stored = if custom {
            origin.as_str().to_owned()
        } else {
            String::new()
        };
        if !self.settings.set(SERVER_KEY.to_owned(), stored) {
            return Err(AccountError::Unavailable);
        }
        self.settings.set(CACHE_KEY.to_owned(), String::new());
        {
            let mut inner = self.lock();
            inner.epoch += 1;
            inner.client = client;
            inner.custom_server = custom;
            inner.config = None;
            inner.config_etag = None;
            inner.config_until = None;
            inner.pending = None;
            inner.clear_account();
            inner.signed_in = signed_in;
        }
        self.notify();
        Ok(())
    }

    fn signed_out_on_401<T>(
        &self,
        epoch: u64,
        result: Result<T, AccountError>,
    ) -> Result<T, AccountError> {
        if result.as_ref().err() == Some(&AccountError::SignedOut) {
            self.forget(epoch);
        }
        result
    }

    /// Drops the session from memory and settings. The vault entry for this
    /// server is cleared too, since a refused bearer will never work again;
    /// other servers' entries are left alone.
    fn forget(&self, epoch: u64) {
        let origin = {
            let mut inner = self.lock();
            if inner.epoch != epoch || !inner.signed_in {
                return;
            }
            inner.epoch += 1;
            inner.signed_in = false;
            inner.clear_account();
            inner.origin()
        };
        let _ = self.vault.clear(&origin);
        self.settings.set(CACHE_KEY.to_owned(), String::new());
        self.notify();
    }

    fn persist(&self, inner: &Inner) {
        let cache = Cache {
            origin: inner.origin(),
            profile: inner.profile.clone(),
            billing: inner.billing.clone(),
            usage: inner.usage.clone(),
        };
        if let Ok(text) = serde_json::to_string(&cache) {
            if text.len() <= MAX_CACHE_BYTES {
                self.settings.set(CACHE_KEY.to_owned(), text);
            }
        }
    }
}

fn plausible_email(email: &str) -> bool {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    email.len() <= MAX_EMAIL_BYTES
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::plausible_email;

    #[test]
    fn emails_need_a_local_part_and_a_dotted_domain() {
        assert!(plausible_email("a@b.co"));
        assert!(plausible_email("first.last+tag@mail.example.org"));
        for refused in ["", "a", "@b.co", "a@b", "a@.co", "a@b.", "a b@c.co"] {
            assert!(!plausible_email(refused), "{refused}");
        }
    }
}
