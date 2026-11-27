//! The optional Zephium account, as the frame sees it. The bearer stays in
//! native code; the frame gets the account's state and closed faults only.
use super::*;
use zephium_account::wire::{self, Billing, PlanChange, Purchase, Receipt, SignupMode, Usage};
use zephium_account::{AccountError, AccountSettings, CallSource, Service, SystemVault, View};

pub(crate) mod avatar;

/// Four a minute, under the service's six, so a refresh by hand still fits.
const CHECKOUT_POLL: std::time::Duration = std::time::Duration::from_secs(15);
/// How often a queued or running plan change is read, and a slower pace for
/// one whose payment is unconfirmed; the wait ends after this long.
const PLAN_CHANGE_POLL: std::time::Duration = std::time::Duration::from_secs(4);
const PLAN_CHANGE_UNCERTAIN: std::time::Duration = std::time::Duration::from_secs(20);
const PLAN_CHANGE_WATCH: std::time::Duration = std::time::Duration::from_secs(12 * 60 * 60);
const MAX_EMAIL_INPUT_BYTES: usize = 320;
const MAX_SERVER_INPUT_BYTES: usize = 512;
const MAX_NAME_INPUT_BYTES: usize = 512;
const MAX_CURSOR_INPUT_BYTES: usize = 256;
/// When to read an unconfirmed hosted call's receipt again: soon, then after
/// the service's recovery has had time to settle or refund it.
const RECEIPT_CHECKS: [std::time::Duration; 3] = [
    std::time::Duration::from_secs(3),
    std::time::Duration::from_secs(30),
    std::time::Duration::from_secs(180),
];

static SERVICE: OnceLock<Arc<Service>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountFaultV1 {
    Offline,
    /// The session ended; sign in again.
    SignedOut,
    RateLimited,
    InvalidCode,
    SignupClosed,
    /// The service, or the part asked for, is off.
    Unavailable,
    /// Nothing has been bought, so there is nothing to manage.
    NoBillingAccount,
    Server,
    /// The Keychain or Credential Manager refused.
    Vault,
    Invalid,
    /// The change needs a fresh sign-in; the session itself is still valid.
    ReauthRequired,
    /// Another account already uses that address.
    EmailTaken,
    NotFound,
    /// Under billing review: no purchase or upgrade; export, the portal and
    /// downgrades still work.
    BillingReview,
    /// A subscription checkout is already open.
    CheckoutInProgress,
    /// There is a subscription already: change it instead of buying another.
    SubscriptionChangeRequired,
    /// Billing is read again before another try.
    BillingRefreshRequired,
    PlanChangeInProgress,
    PlanChangeExpired,
    PlanChangeUnavailable,
    /// The picture is missing, empty or could not be read.
    AvatarInvalid,
    /// The picture's upload took too long.
    AvatarTimeout,
    /// The picture is over 2 MiB.
    AvatarTooLarge,
    /// Not a still PNG, JPEG or WebP picture.
    AvatarUnsupported,
    /// The picture has more pixels than the service takes.
    AvatarDimensions,
    /// The service could not convert the picture.
    AvatarProcessing,
    /// The service is busy with pictures; try again after `retry_after`.
    AvatarBusy,
    /// The upload was stopped here; nothing to tell the person.
    AvatarCancelled,
}

impl From<AccountError> for AccountFaultV1 {
    fn from(error: AccountError) -> Self {
        match error {
            AccountError::Offline => Self::Offline,
            AccountError::SignedOut => Self::SignedOut,
            AccountError::RateLimited { .. } => Self::RateLimited,
            AccountError::InvalidCode => Self::InvalidCode,
            AccountError::SignupClosed => Self::SignupClosed,
            AccountError::Unavailable => Self::Unavailable,
            AccountError::NoBillingAccount => Self::NoBillingAccount,
            AccountError::Server | AccountError::Protocol => Self::Server,
            AccountError::Vault => Self::Vault,
            AccountError::Invalid => Self::Invalid,
            AccountError::ReauthRequired => Self::ReauthRequired,
            AccountError::EmailTaken => Self::EmailTaken,
            AccountError::NotFound => Self::NotFound,
            AccountError::BillingReview => Self::BillingReview,
            AccountError::CheckoutInProgress => Self::CheckoutInProgress,
            AccountError::SubscriptionChangeRequired => Self::SubscriptionChangeRequired,
            AccountError::BillingRefreshRequired => Self::BillingRefreshRequired,
            AccountError::PlanChangeInProgress => Self::PlanChangeInProgress,
            AccountError::PlanChangeExpired => Self::PlanChangeExpired,
            AccountError::PlanChangeUnavailable => Self::PlanChangeUnavailable,
            AccountError::AvatarInvalid => Self::AvatarInvalid,
            AccountError::AvatarTimedOut => Self::AvatarTimeout,
            AccountError::AvatarTooLarge => Self::AvatarTooLarge,
            AccountError::AvatarUnsupported => Self::AvatarUnsupported,
            AccountError::AvatarDimensions => Self::AvatarDimensions,
            AccountError::AvatarProcessing => Self::AvatarProcessing,
            AccountError::AvatarBusy { .. } => Self::AvatarBusy,
            AccountError::AvatarCancelled => Self::AvatarCancelled,
        }
    }
}

/// Usage as the server counts it. Amounts are decimal strings of integer
/// units, `unit_scale` to one credit; they are shown divided, never summed
/// into a charge. Held units are for calls still running.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountUsageV1 {
    /// Share of the plan's allowance used or held, 0–100; null without an allowance.
    pub(crate) used_percent: Option<u8>,
    /// When the allowance renews, as the server states it (ISO 8601).
    pub(crate) resets_at: Option<String>,
    /// Bought usage remains beyond the plan's allowance.
    pub(crate) extra: bool,
    pub(crate) unit_scale: u32,
    pub(crate) remaining: Option<String>,
    pub(crate) allowance_total: Option<String>,
    pub(crate) allowance_used: Option<String>,
    pub(crate) held: Option<String>,
    pub(crate) bonus_remaining: Option<String>,
    pub(crate) bonus_expires_at: Option<String>,
    pub(crate) purchased_remaining: Option<String>,
    pub(crate) spent_today: Option<String>,
    /// The daily cap; null when there is none.
    pub(crate) daily_limit: Option<String>,
    /// Units to one credit as the server states it. Null: show percentages
    /// only, never credit numbers.
    pub(crate) credit_units: Option<String>,
    /// What the plan's allowance still holds, in units.
    pub(crate) allowance_left: Option<String>,
    /// The allowance is a one-time grant that never refills.
    pub(crate) one_time: bool,
    /// The free allowance is used up for now; purchased credit still works.
    pub(crate) free_paused: bool,
    /// Purchased credit can be spent while the free allowance is paused.
    pub(crate) purchased_usable: Option<bool>,
}

/// One hosted request's accounting; never its content.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountRequestV1 {
    pub(crate) id: String,
    pub(crate) model_id: Option<String>,
    /// `settled` is final; anything else is still held or being checked.
    pub(crate) state: String,
    /// Charged, in units; final only when settled.
    pub(crate) charged: Option<String>,
    pub(crate) reserved: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) error_class: Option<String>,
    /// Which balance paid: `allowance`, `purchased` or `mixed`.
    pub(crate) funding: Option<String>,
    /// What the call was made for, from this computer's own record.
    pub(crate) source: Option<AccountCallSourceV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AccountCallSourceV1 {
    Task { profile: String, work: String },
    Voice,
    Page,
}

impl From<CallSource> for AccountCallSourceV1 {
    fn from(source: CallSource) -> Self {
        match source {
            CallSource::Task { profile, work } => Self::Task { profile, work },
            CallSource::Voice => Self::Voice,
            CallSource::Page => Self::Page,
        }
    }
}

/// How the server takes new accounts.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, specta::Type, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountSignupModeV1 {
    Open,
    /// By invitation only.
    Allowlist,
    Closed,
    Unknown,
}

/// A Pro/Max change: a quote to confirm, or the operation after it.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountPlanChangeV1 {
    pub(crate) id: String,
    /// `quoted`, `queued`, `executing`, `uncertain`, `applied`, `scheduled` or `failed`.
    pub(crate) status: String,
    pub(crate) current_plan: String,
    pub(crate) target_plan: String,
    pub(crate) interval: String,
    /// `upgrade` or `downgrade`.
    pub(crate) direction: String,
    /// Null for an immediate upgrade; the renewal for a downgrade.
    pub(crate) effective_at: Option<String>,
    /// An estimate before tax; the provider decides the final charge.
    pub(crate) charge: Option<AccountPriceV1>,
    pub(crate) recurring: Option<AccountPriceV1>,
    pub(crate) expires_at: Option<String>,
    /// The stable code of a failed change.
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountHistoryV1 {
    pub(crate) requests: Vec<AccountRequestV1>,
    /// Opaque; passed back unchanged for the next page.
    pub(crate) next_cursor: Option<String>,
    pub(crate) fault: Option<AccountFaultV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountReceiptV1 {
    pub(crate) request: Option<AccountRequestV1>,
    pub(crate) fault: Option<AccountFaultV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountExportV1 {
    /// The export was written where the person chose.
    pub(crate) saved: bool,
    pub(crate) fault: Option<AccountFaultV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountSubscriptionV1 {
    /// `active`, `inactive` or `unknown`.
    pub(crate) status: String,
    /// The paid plan held; null with no paid subscription. Model access follows `plan`, not this.
    pub(crate) plan: Option<String>,
    /// `monthly` or `yearly`; null with no paid subscription.
    pub(crate) interval: Option<String>,
    pub(crate) paid_until: Option<String>,
    pub(crate) cancel_at_period_end: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountPriceV1 {
    /// In the currency's minor unit, as the billing provider states it.
    pub(crate) amount_minor: u32,
    pub(crate) currency: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AccountOfferV1 {
    Plan {
        plan: String,
        interval: String,
        name: Option<String>,
        price: Option<AccountPriceV1>,
        features: Vec<String>,
    },
    TopUp {
        id: String,
        name: Option<String>,
        /// Credit it adds, in units of the usage's scale.
        units: Option<String>,
        price: Option<AccountPriceV1>,
        expires_days: Option<u32>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountNoticeV1 {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) warning: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountV1 {
    pub(crate) version: u16,
    /// Accounts work on this platform and build.
    pub(crate) available: bool,
    pub(crate) server: String,
    pub(crate) custom_server: bool,
    /// What the server offers; null until it has answered once.
    pub(crate) hosted_models: Option<bool>,
    /// Purchases and the billing portal are offered here.
    pub(crate) billing: Option<bool>,
    pub(crate) signup_open: Option<bool>,
    /// The server asks for a newer Zephium than this one.
    pub(crate) update_required: bool,
    pub(crate) signed_in: bool,
    /// Signed in, but the Keychain or Credential Manager will not give up the
    /// session, so hosted calls cannot run until the person signs in again.
    pub(crate) vault_fault: bool,
    /// The address a sign-in code was sent to, while waiting for it.
    pub(crate) pending_email: Option<String>,
    pub(crate) resend_in: Option<u32>,
    pub(crate) email: Option<String>,
    pub(crate) name: Option<String>,
    /// The account picture's revision, served by `zephium-media` at
    /// `account-avatar/<revision>`; null shows initials. Never the service's
    /// address for it.
    pub(crate) avatar: Option<u32>,
    pub(crate) plan: Option<String>,
    pub(crate) usage: Option<AccountUsageV1>,
    pub(crate) subscription: Option<AccountSubscriptionV1>,
    pub(crate) offers: Vec<AccountOfferV1>,
    /// A checkout was opened and is being watched for completion.
    pub(crate) checkout_open: bool,
    /// The billing portal was opened and billing is being read until it changes.
    pub(crate) portal_open: bool,
    pub(crate) signup_mode: Option<AccountSignupModeV1>,
    /// Under billing review: a calm state in place of purchase buttons.
    pub(crate) billing_review: bool,
    pub(crate) plan_change: Option<AccountPlanChangeV1>,
    pub(crate) notice: Option<AccountNoticeV1>,
    /// Hosted calls whose charge was not confirmed when they ended.
    pub(crate) unsettled: Vec<AccountRequestV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountReplyV1 {
    pub(crate) account: AccountV1,
    pub(crate) fault: Option<AccountFaultV1>,
    /// Seconds to wait before trying again, with `rate_limited` or `avatar_busy`.
    pub(crate) retry_after: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountSessionV1 {
    pub(crate) id: String,
    pub(crate) device: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) last_used_at: Option<String>,
    pub(crate) current: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub(crate) struct AccountSessionsV1 {
    pub(crate) sessions: Vec<AccountSessionV1>,
    pub(crate) fault: Option<AccountFaultV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AccountPurchaseV1 {
    Plan { plan: String, interval: String },
    TopUp { id: String },
}

/// The account changed; read it again.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "zephium:account-changed")]
pub(crate) struct AccountChanged {
    pub(crate) version: u16,
}

struct AppSettings;

impl AccountSettings for AppSettings {
    fn get(&self, key: &str) -> Option<String> {
        APP_STORE.get().and_then(|store| store.app_setting(key))
    }
    fn set(&self, key: String, value: String) -> bool {
        APP_STORE
            .get()
            .is_some_and(|store| store.set_app_setting(key, value))
    }
}

/// A label the person recognises in their list of signed-in devices. It names
/// the app and the system only: no host name, model or user name.
fn device_label(app: &tauri::AppHandle) -> String {
    let system = if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "Linux"
    };
    format!("Zephium {} · {system}", app.package_info().version)
}

/// Creates the account service after the app store exists. A session left in
/// the vault is refreshed in the background.
pub(crate) fn install(app: &tauri::AppHandle) {
    let service = match Service::new(
        Arc::new(AppSettings),
        Arc::new(SystemVault),
        &device_label(app),
        &app.package_info().version.to_string(),
    ) {
        Ok(service) => Arc::new(service),
        Err(error) => {
            write_diagnostic(format_args!("account: install fault={error:?}"));
            return;
        }
    };
    let events = app.clone();
    service.set_observer(Box::new(move || {
        emit_to_privileged(
            &events,
            MAIN_LABEL,
            "zephium:account-changed",
            &AccountChanged { version: 1 },
        );
        #[cfg(feature = "work-product")]
        hosted::account_changed();
    }));
    if SERVICE.set(service.clone()).is_err() {
        return;
    }
    const SCOPED_KEY: &str = "account.vault.scoped";
    if AppSettings.get(SCOPED_KEY).is_none() && AppSettings.set(SCOPED_KEY.into(), "1".into()) {
        std::thread::spawn(|| SystemVault.drop_unscoped());
    }
    #[cfg(feature = "work-product")]
    hosted::install(service.clone());
    // Nobody who has not chosen an account, or another server, is ever
    // connected to one: the server is asked only once they have.
    if !contacted(&service) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let _ = service.load_config().await;
        if let Err(error) = service.refresh(true).await {
            write_diagnostic(format_args!("account: refresh fault={error:?}"));
        }
        // A plan change started before the app closed is followed to its end.
        if service.plan_change_open() {
            watch_plan_change(service);
        }
    });
}

/// Whether the person has chosen to involve an account server.
fn contacted(service: &Service) -> bool {
    service.signed_in() || service.view().custom_server
}

/// The signed-in account as Work's hosted model provider.
#[cfg(feature = "work-product")]
mod hosted {
    use super::*;
    use zephium_agentic::lead::{LeadCloudRecord, LeadCredential, LeadSecret, LeadSecretFuture};
    use zephium_app::work_models::{self, WorkCallSource, WorkCloudSession};
    use zephium_core::work::model::WorkModelError;

    static SIGNED_IN: AtomicBool = AtomicBool::new(false);

    struct Hosted(Arc<Service>);

    impl WorkCloudSession for Hosted {
        fn signed_in(&self) -> bool {
            self.0.signed_in()
        }
        fn plan(&self) -> Option<String> {
            self.0.plan()
        }
        fn base(&self) -> String {
            self.0.origin()
        }
        fn credential(&self) -> Arc<dyn LeadCredential> {
            Arc::new(Bearer(self.0.clone()))
        }
        fn sourced(&self, request_id: &str, source: &WorkCallSource) {
            self.0.record_call_source(
                request_id,
                match source {
                    WorkCallSource::Task { profile, work } => CallSource::Task {
                        profile: profile.clone(),
                        work: work.clone(),
                    },
                    WorkCallSource::Voice => CallSource::Voice,
                    WorkCallSource::Page {
                        conversation,
                        reply,
                    } => {
                        super::super::page_assist::hosted_sourced(request_id, conversation, *reply);
                        CallSource::Page
                    }
                },
            );
        }
        fn recorded(&self, record: LeadCloudRecord) {
            self.0.usage_changed();
            let per_credit =
                |service: &Service| service.view().usage.and_then(|usage| usage.credit_units());
            let unsettled = record.unsettled();
            if let (Some(id), zephium_agentic::lead::LeadCloudAccounting::Settled { charged }) =
                (&record.request_id, &record.accounting)
            {
                super::super::page_assist::hosted_charged(
                    id,
                    charged.as_deref().and_then(|units| units.parse().ok()),
                    per_credit(&self.0),
                );
            }
            let Some(id) = record.request_id.filter(|_| unsettled) else {
                return;
            };
            let service = self.0.clone();
            // Read the receipt; a hosted call is never sent again on its own.
            tauri::async_runtime::spawn(async move {
                for wait in RECEIPT_CHECKS {
                    tokio::time::sleep(wait).await;
                    match service.confirm_request(&id).await {
                        Ok(receipt) if receipt.settled() => {
                            super::super::page_assist::hosted_charged(
                                &id,
                                receipt
                                    .charged
                                    .as_deref()
                                    .and_then(|units| units.parse().ok()),
                                per_credit(&service),
                            );
                            return;
                        }
                        Err(AccountError::SignedOut | AccountError::NotFound) => return,
                        _ => {}
                    }
                }
            });
        }
    }

    struct Bearer(Arc<Service>);

    impl LeadCredential for Bearer {
        fn secret(&self) -> LeadSecretFuture<'_> {
            let service = self.0.clone();
            Box::pin(async move {
                // The first read after launch may touch the vault.
                let token = tokio::task::spawn_blocking(move || service.bearer())
                    .await
                    .map_err(|_| WorkModelError::MissingKey)?
                    .map_err(|_| WorkModelError::MissingKey)?;
                LeadSecret::new(token.expose().to_owned())
                    .map(Arc::new)
                    .map_err(|_| WorkModelError::MissingKey)
            })
        }

        fn rejected(&self) {
            self.0.bearer_refused();
        }
    }

    pub(super) fn install(service: Arc<Service>) {
        SIGNED_IN.store(service.signed_in(), Ordering::Release);
        work_models::install_cloud_session(Arc::new(Hosted(service)));
        refresh_catalog();
    }

    /// Only a sign-in or sign-out changes what the picker offers.
    pub(super) fn account_changed() {
        let Some(service) = super::service() else {
            return;
        };
        let now = service.signed_in();
        if SIGNED_IN.swap(now, Ordering::AcqRel) != now {
            work_models::cloud_session_changed();
            refresh_catalog();
        }
    }

    fn refresh_catalog() {
        tauri::async_runtime::spawn(work_models::refresh_cloud_catalog());
    }
}

pub(crate) fn service() -> Option<Arc<Service>> {
    SERVICE.get().cloned()
}

fn available() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows")) && SERVICE.get().is_some()
}

fn project(view: View) -> AccountV1 {
    let config = view.config.as_ref();
    let billing = view.billing.as_ref();
    // Until Zephium's own server has been asked, what it offers is known.
    let ours = (!view.custom_server).then_some(true);
    // The signed-in account's own answers outrank the public hints.
    let hosted_models = view
        .usage
        .as_ref()
        .map(|usage| usage.cloud_enabled)
        .or(billing
            .map(|billing| billing.capabilities.hosted_ai)
            .filter(|on| *on))
        .or(config.map(|c| c.cloud_enabled))
        .or(ours);
    let sells = config
        .map(|c| c.billing_enabled)
        .or(ours)
        .map(|on| on && billing.is_none_or(Billing::sells));
    AccountV1 {
        version: 1,
        available: available(),
        server: view.server,
        custom_server: view.custom_server,
        hosted_models,
        billing: sells,
        signup_open: config.map(|c| c.signup_open).or(ours),
        update_required: view.update_required,
        signed_in: view.signed_in,
        vault_fault: view.vault_fault,
        pending_email: view.pending_email,
        resend_in: view.resend_in,
        email: view.profile.as_ref().map(|p| p.email.clone()),
        name: view.profile.as_ref().and_then(|p| p.name.clone()),
        avatar: view.avatar,
        plan: view.usage.as_ref().and_then(|u| u.plan.clone()),
        usage: view.usage.as_ref().map(usage),
        subscription: billing.map(|billing| AccountSubscriptionV1 {
            status: billing.subscription.status.clone(),
            plan: billing.subscription.plan.clone(),
            interval: billing.subscription.interval.clone(),
            paid_until: billing.subscription.paid_until.clone(),
            cancel_at_period_end: billing.subscription.cancel_at_period_end,
        }),
        offers: billing
            .filter(|billing| billing.sells())
            .map(offers)
            .unwrap_or_default(),
        checkout_open: view.checkout_open,
        portal_open: view.portal_open,
        signup_mode: config.and_then(|c| c.signup_mode).map(signup_mode),
        billing_review: view.billing_review,
        plan_change: view.plan_change.as_ref().map(plan_change),
        notice: config
            .and_then(|c| c.notice.as_ref())
            .map(|notice| AccountNoticeV1 {
                id: notice.id.clone(),
                text: notice.text.clone(),
                warning: notice.severity.as_deref() == Some("warning"),
            }),
        unsettled: view.unsettled.iter().map(request).collect(),
    }
}

fn signup_mode(mode: SignupMode) -> AccountSignupModeV1 {
    match mode {
        SignupMode::Open => AccountSignupModeV1::Open,
        SignupMode::Allowlist => AccountSignupModeV1::Allowlist,
        SignupMode::Closed => AccountSignupModeV1::Closed,
        SignupMode::Unknown => AccountSignupModeV1::Unknown,
    }
}

fn plan_change(change: &PlanChange) -> AccountPlanChangeV1 {
    AccountPlanChangeV1 {
        id: change.id.clone(),
        status: change.status.clone(),
        current_plan: change.current_plan.clone(),
        target_plan: change.target_plan.clone(),
        interval: change.interval.clone(),
        direction: change.direction.clone(),
        effective_at: change.effective_at.clone(),
        charge: price(&change.amount_due_estimate),
        recurring: price(&change.recurring_price),
        expires_at: change.expires_at.clone(),
        error: change.error.as_ref().map(|fault| fault.code.clone()),
    }
}

fn usage(usage: &Usage) -> AccountUsageV1 {
    let units = |value: &Option<String>| wire::amount(value.as_deref());
    AccountUsageV1 {
        used_percent: usage.used_percent(),
        // A one-time allowance never renews, whatever a date field says.
        resets_at: usage
            .period_end
            .clone()
            .filter(|_| !usage.one_time_allowance()),
        credit_units: usage.credit_units().map(|units| units.to_string()),
        allowance_left: usage.allowance_left(),
        one_time: usage.one_time_allowance(),
        free_paused: usage.free_usage_paused,
        purchased_usable: usage.purchased_usage_available,
        extra: usage.has_extra(),
        unit_scale: u32::try_from(usage.scale()).unwrap_or(1_000_000),
        remaining: units(&usage.credits_remaining),
        allowance_total: units(&usage.allowance.total_units),
        allowance_used: units(&usage.allowance.used_units),
        held: units(&usage.reserved_units).or_else(|| units(&usage.allowance.reserved_units)),
        bonus_remaining: units(&usage.bonus_remaining),
        bonus_expires_at: usage.bonus_expires_at.clone(),
        purchased_remaining: units(&usage.purchased_remaining),
        spent_today: units(&usage.spent_today),
        daily_limit: units(&usage.daily_limit_units),
    }
}

fn request(receipt: &Receipt) -> AccountRequestV1 {
    AccountRequestV1 {
        id: receipt.id.clone(),
        model_id: receipt.model_id.clone(),
        state: receipt.state.clone(),
        charged: wire::amount(receipt.charged.as_deref()),
        reserved: wire::amount(receipt.reserved.as_deref()),
        started_at: receipt.started_at.clone(),
        finished_at: receipt.finished_at.clone(),
        error_class: receipt.error_class.clone(),
        funding: funding(receipt.funding_source.as_deref()),
        source: None,
    }
}

/// The funding source the contract names; anything else reads as unknown.
fn funding(source: Option<&str>) -> Option<String> {
    source
        .filter(|source| matches!(*source, "allowance" | "purchased" | "mixed"))
        .map(str::to_owned)
}

fn price(price: &Option<zephium_account::wire::Price>) -> Option<AccountPriceV1> {
    let price = price.as_ref()?;
    Some(AccountPriceV1 {
        amount_minor: u32::try_from(price.amount_minor).ok()?,
        currency: price.currency.clone(),
    })
}

fn offers(billing: &Billing) -> Vec<AccountOfferV1> {
    let Some(offers) = &billing.offers else {
        return Vec::new();
    };
    let plans = offers
        .subscriptions
        .iter()
        .map(|offer| AccountOfferV1::Plan {
            plan: offer.plan.clone(),
            interval: offer.interval.clone(),
            name: offer.product_name.clone(),
            price: price(&offer.price),
            features: offer.features.clone(),
        });
    let topups = offers.topups.iter().map(|offer| AccountOfferV1::TopUp {
        id: offer.id.clone(),
        name: offer.product_name.clone(),
        units: wire::amount(offer.units.as_deref()),
        price: price(&offer.price),
        expires_days: offer.expires_days,
    });
    plans.chain(topups).collect()
}

fn unavailable_view() -> AccountV1 {
    AccountV1 {
        version: 1,
        available: false,
        server: zephium_account::DEFAULT_ORIGIN.to_owned(),
        custom_server: false,
        hosted_models: None,
        billing: None,
        signup_open: None,
        update_required: false,
        signed_in: false,
        vault_fault: false,
        pending_email: None,
        resend_in: None,
        email: None,
        name: None,
        avatar: None,
        plan: None,
        usage: None,
        subscription: None,
        offers: Vec::new(),
        checkout_open: false,
        portal_open: false,
        signup_mode: None,
        billing_review: false,
        plan_change: None,
        notice: None,
        unsettled: Vec::new(),
    }
}

fn reply(result: Result<(), AccountError>) -> AccountReplyV1 {
    let account = service()
        .map(|service| project(service.view()))
        .unwrap_or_else(unavailable_view);
    let (fault, retry_after) = match result {
        Ok(()) => (None, None),
        Err(AccountError::RateLimited { retry_after }) => {
            (Some(AccountFaultV1::RateLimited), Some(retry_after))
        }
        Err(AccountError::AvatarBusy { retry_after }) => {
            (Some(AccountFaultV1::AvatarBusy), Some(retry_after))
        }
        Err(error) => (Some(error.into()), None),
    };
    AccountReplyV1 {
        account,
        fault,
        retry_after,
    }
}

fn admitted(caller: &WebviewWindow, label: &str) -> Result<Arc<Service>, AccountError> {
    if !authorize(caller, CallerPolicy::Main, label) {
        return Err(AccountError::Invalid);
    }
    service().ok_or(AccountError::Unavailable)
}

fn log(action: &str, result: &Result<(), AccountError>) {
    if let Err(error) = result {
        write_diagnostic(format_args!("account: action={action} fault={error:?}"));
    }
}

/// The account now, refreshing what is stale in the background. Asks no
/// server unless the person is signed in or chose one.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_get(caller: WebviewWindow) -> AccountReplyV1 {
    let service = match admitted(&caller, "account_get") {
        Ok(service) => service,
        Err(error) => return reply(Err(error)),
    };
    if contacted(&service) {
        let background = service.clone();
        tauri::async_runtime::spawn(async move {
            if background.view().config.is_none() {
                let _ = background.load_config().await;
            }
            let _ = background.refresh(false).await;
            // Shown means revalidated: a picture changed on another device appears here.
            let _ = background.load_avatar().await;
            if background.plan_change_in_flight() {
                watch_plan_change(background);
            }
        });
    }
    reply(Ok(()))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn account_send_code(caller: WebviewWindow, email: String) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_send_code") {
        Ok(_) if email.len() > MAX_EMAIL_INPUT_BYTES => Err(AccountError::Invalid),
        Ok(service) => service.send_code(&email).await,
        Err(error) => Err(error),
    };
    log("send_code", &result);
    reply(result)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn account_verify(caller: WebviewWindow, code: String) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_verify") {
        Ok(_) if code.len() > 16 => Err(AccountError::InvalidCode),
        Ok(service) => service.verify(&code).await,
        Err(error) => Err(error),
    };
    log("verify", &result);
    reply(result)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn account_cancel_code(caller: WebviewWindow) -> AccountReplyV1 {
    reply(admitted(&caller, "account_cancel_code").map(|service| service.cancel_code()))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn account_sign_out(caller: WebviewWindow) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_sign_out") {
        Ok(service) => service.sign_out().await,
        Err(error) => Err(error),
    };
    log("sign_out", &result);
    reply(result)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn account_refresh(caller: WebviewWindow) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_refresh") {
        Ok(service) => {
            let _ = service.load_config().await;
            service.refresh(true).await
        }
        Err(error) => Err(error),
    };
    log("refresh", &result);
    reply(result)
}

/// Opens the billing provider's checkout in a new tab and watches for the
/// purchase to land, so the plan updates without the person coming back.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_buy(
    caller: WebviewWindow,
    purchase: AccountPurchaseV1,
) -> AccountReplyV1 {
    let purchase = match purchase {
        AccountPurchaseV1::Plan { plan, interval } => Purchase::Plan { plan, interval },
        AccountPurchaseV1::TopUp { id } => Purchase::TopUp { id },
    };
    let result = match admitted(&caller, "account_buy") {
        Ok(service) => match service.checkout(&purchase).await {
            Ok(url) => {
                open_tab(&caller, url.as_str());
                watch_checkout(service);
                Ok(())
            }
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    log("buy", &result);
    reply(result)
}

/// Opens the billing provider's portal, where the subscription is managed.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_manage(caller: WebviewWindow) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_manage") {
        Ok(service) => match service.portal().await {
            Ok(url) => {
                open_tab(&caller, url.as_str());
                Ok(())
            }
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    log("manage", &result);
    reply(result)
}

/// Sets the daily spending limit, in units, or removes it with null.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_set_daily_limit(
    caller: WebviewWindow,
    limit: Option<String>,
) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_set_daily_limit") {
        Ok(_) if limit.as_ref().is_some_and(|limit| limit.len() > 20) => Err(AccountError::Invalid),
        Ok(service) => service.set_daily_limit(limit.as_deref()).await,
        Err(error) => Err(error),
    };
    log("set_daily_limit", &result);
    reply(result)
}

/// Asks for a quote to move to another paid plan. Nothing is charged.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_plan_preview(
    caller: WebviewWindow,
    plan: String,
    interval: String,
) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_plan_preview") {
        Ok(_) if plan.len() > 64 || interval.len() > 16 => Err(AccountError::Invalid),
        Ok(service) => service
            .preview_plan_change(&plan, &interval)
            .await
            .map(drop),
        Err(error) => Err(error),
    };
    log("plan_preview", &result);
    reply(result)
}

/// Confirms the quote on the account page and follows the change to its end.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_plan_confirm(caller: WebviewWindow) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_plan_confirm") {
        Ok(service) => match service.confirm_plan_change().await {
            Ok(_) => {
                watch_plan_change(service);
                Ok(())
            }
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    log("plan_confirm", &result);
    reply(result)
}

/// Puts away a quote or a finished change.
#[tauri::command]
#[specta::specta]
pub(crate) fn account_plan_dismiss(caller: WebviewWindow) -> AccountReplyV1 {
    reply(admitted(&caller, "account_plan_dismiss").map(|service| service.dismiss_plan_change()))
}

/// Reads a held plan change until it ends. `uncertain` is waited on, never
/// paid again; the wait is slower and survives a long outage.
fn watch_plan_change(service: Arc<Service>) {
    static WATCHING: AtomicBool = AtomicBool::new(false);
    if WATCHING.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let started = std::time::Instant::now();
        while started.elapsed() < PLAN_CHANGE_WATCH {
            let wait = match service.refresh_plan_change().await {
                Ok(Some(change)) if change.in_flight() => {
                    if change.status == "uncertain" {
                        PLAN_CHANGE_UNCERTAIN
                    } else {
                        PLAN_CHANGE_POLL
                    }
                }
                // A quote is read once; there is nothing to wait for.
                Ok(_) => break,
                Err(AccountError::SignedOut | AccountError::NotFound) => break,
                Err(AccountError::RateLimited { retry_after }) => {
                    std::time::Duration::from_secs(retry_after.into())
                }
                Err(_) => PLAN_CHANGE_UNCERTAIN,
            };
            tokio::time::sleep(wait).await;
        }
        WATCHING.store(false, Ordering::Release);
    });
}

fn open_tab(caller: &WebviewWindow, url: &str) {
    let app = caller.app_handle();
    let shell = app.state::<Handle>();
    let _ = dispatch_operation(
        app,
        &shell,
        Command::OpenUrl {
            input: url.to_owned(),
            new_tab: true,
        },
    );
}

fn watch_checkout(service: Arc<Service>) {
    static WATCHING: AtomicBool = AtomicBool::new(false);
    if WATCHING.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        while service.checkout_open() {
            tokio::time::sleep(CHECKOUT_POLL).await;
            match service.refresh_billing().await {
                Err(AccountError::SignedOut) => break,
                Err(AccountError::RateLimited { retry_after }) => {
                    tokio::time::sleep(std::time::Duration::from_secs(retry_after.into())).await;
                }
                _ => {}
            }
        }
        WATCHING.store(false, Ordering::Release);
    });
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn account_sessions(caller: WebviewWindow) -> AccountSessionsV1 {
    let result = match admitted(&caller, "account_sessions") {
        Ok(service) => service.sessions().await,
        Err(error) => Err(error),
    };
    match result {
        Ok(sessions) => AccountSessionsV1 {
            sessions: sessions
                .into_iter()
                .map(|session| AccountSessionV1 {
                    id: session.id,
                    device: session.device,
                    created_at: session.created_at,
                    last_used_at: session.updated_at,
                    current: session.current,
                })
                .collect(),
            fault: None,
        },
        Err(error) => AccountSessionsV1 {
            sessions: Vec::new(),
            fault: Some(error.into()),
        },
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn account_revoke_session(caller: WebviewWindow, id: String) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_revoke_session") {
        Ok(_) if id.len() > 128 => Err(AccountError::Invalid),
        Ok(service) => service.revoke_session(&id).await,
        Err(error) => Err(error),
    };
    log("revoke_session", &result);
    reply(result)
}

/// Points accounts at another server, or back at Zephium's with null.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_set_server(
    caller: WebviewWindow,
    server: Option<String>,
) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_set_server") {
        Ok(_)
            if server
                .as_ref()
                .is_some_and(|s| s.len() > MAX_SERVER_INPUT_BYTES) =>
        {
            Err(AccountError::Invalid)
        }
        Ok(service) => match service.set_server(server.as_deref()).await {
            Ok(()) => {
                let _ = service.load_config().await;
                Ok(())
            }
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    log("set_server", &result);
    reply(result)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn account_deletion_code(caller: WebviewWindow) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_deletion_code") {
        Ok(service) => service.send_deletion_code().await,
        Err(error) => Err(error),
    };
    log("deletion_code", &result);
    reply(result)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn account_delete(caller: WebviewWindow, code: String) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_delete") {
        Ok(_) if code.len() > 16 => Err(AccountError::InvalidCode),
        Ok(service) => service.delete_account(&code).await,
        Err(error) => Err(error),
    };
    log("delete", &result);
    reply(result)
}

/// Renames the account; null or blank clears the name.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_set_name(
    caller: WebviewWindow,
    name: Option<String>,
) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_set_name") {
        Ok(_)
            if name
                .as_ref()
                .is_some_and(|n| n.len() > MAX_NAME_INPUT_BYTES) =>
        {
            Err(AccountError::Invalid)
        }
        Ok(service) => service.set_name(name.as_deref()).await,
        Err(error) => Err(error),
    };
    log("set_name", &result);
    reply(result)
}

/// Sends a sign-in code to the account's own address, for a change that
/// needs a session from the last few minutes.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_reauth_code(caller: WebviewWindow) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_reauth_code") {
        Ok(service) => service.send_reauth_code().await,
        Err(error) => Err(error),
    };
    log("reauth_code", &result);
    reply(result)
}

/// Sends a code to the address the account would move to.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_email_code(caller: WebviewWindow, email: String) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_email_code") {
        Ok(_) if email.len() > MAX_EMAIL_INPUT_BYTES => Err(AccountError::Invalid),
        Ok(service) => service.send_email_change_code(&email).await,
        Err(error) => Err(error),
    };
    log("email_code", &result);
    reply(result)
}

/// Moves the account to `email`. Every session ends, this one included.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_change_email(
    caller: WebviewWindow,
    email: String,
    code: String,
) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_change_email") {
        Ok(_) if email.len() > MAX_EMAIL_INPUT_BYTES || code.len() > 16 => {
            Err(AccountError::Invalid)
        }
        Ok(service) => service.change_email(&email, &code).await,
        Err(error) => Err(error),
    };
    log("change_email", &result);
    reply(result)
}

/// Ends every session of the account, on every device.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_sign_out_everywhere(caller: WebviewWindow) -> AccountReplyV1 {
    let result = match admitted(&caller, "account_sign_out_everywhere") {
        Ok(service) => service.sign_out_everywhere().await,
        Err(error) => Err(error),
    };
    log("sign_out_everywhere", &result);
    reply(result)
}

/// Saves the server's account export where the person chooses. The file
/// holds what the server sent: no bearer, no browsing data.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_export(caller: WebviewWindow) -> AccountExportV1 {
    use tauri_plugin_dialog::DialogExt;
    let refused = |error: AccountError| AccountExportV1 {
        saved: false,
        fault: Some(error.into()),
    };
    let service = match admitted(&caller, "account_export") {
        Ok(service) => service,
        Err(error) => return refused(error),
    };
    let export = match service.export().await {
        Ok(export) => export,
        Err(error) => {
            log("export", &Err(error));
            return refused(error);
        }
    };
    let Ok(bytes) = serde_json::to_vec_pretty(&export) else {
        return refused(AccountError::Server);
    };
    let app = caller.app_handle().clone();
    let picked = tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name("Zephium account.json")
            .add_filter("JSON", &["json"])
            .blocking_save_file()
    })
    .await
    .ok()
    .flatten();
    let Some(path) = picked.and_then(|picked| picked.into_path().ok()) else {
        return AccountExportV1 {
            saved: false,
            fault: None,
        };
    };
    match tokio::task::spawn_blocking(move || std::fs::write(path, bytes)).await {
        Ok(Ok(())) => AccountExportV1 {
            saved: true,
            fault: None,
        },
        _ => refused(AccountError::Invalid),
    }
}

/// One page of the account's hosted requests, newest first.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_usage_history(
    caller: WebviewWindow,
    cursor: Option<String>,
) -> AccountHistoryV1 {
    let result = match admitted(&caller, "account_usage_history") {
        Ok(_)
            if cursor
                .as_ref()
                .is_some_and(|c| c.len() > MAX_CURSOR_INPUT_BYTES) =>
        {
            Err(AccountError::Invalid)
        }
        Ok(service) => service.usage_history(cursor.as_deref()).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(page) => {
            let ids: Vec<String> = page.requests.iter().map(|row| row.id.clone()).collect();
            let sources = service()
                .map(|service| service.call_sources(&ids))
                .unwrap_or_default();
            AccountHistoryV1 {
                requests: page
                    .requests
                    .into_iter()
                    .enumerate()
                    .map(|(index, row)| AccountRequestV1 {
                        id: row.id,
                        model_id: row.model_id,
                        state: row.state,
                        charged: wire::amount(row.units.as_deref()),
                        reserved: wire::amount(row.reserved.as_deref()),
                        started_at: row.started_at,
                        finished_at: row.finished_at,
                        error_class: None,
                        funding: funding(row.funding_source.as_deref()),
                        source: sources.get(index).cloned().flatten().map(Into::into),
                    })
                    .collect(),
                next_cursor: page.next_cursor,
                fault: None,
            }
        }
        Err(error) => {
            log("usage_history", &Err(error));
            AccountHistoryV1 {
                requests: Vec::new(),
                next_cursor: None,
                fault: Some(error.into()),
            }
        }
    }
}

/// One hosted request's receipt.
#[tauri::command]
#[specta::specta]
pub(crate) async fn account_receipt(caller: WebviewWindow, id: String) -> AccountReceiptV1 {
    let result = match admitted(&caller, "account_receipt") {
        Ok(_) if id.len() > 64 => Err(AccountError::Invalid),
        Ok(service) => service.receipt(&id).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(receipt) => AccountReceiptV1 {
            request: Some(request(&receipt)),
            fault: None,
        },
        Err(error) => AccountReceiptV1 {
            request: None,
            fault: Some(error.into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faults_and_offers_have_stable_wire_shapes() {
        assert_eq!(
            serde_json::to_value(AccountFaultV1::NoBillingAccount).unwrap(),
            "no_billing_account"
        );
        let offer = AccountOfferV1::TopUp {
            id: "small".into(),
            name: None,
            units: Some("5000000".into()),
            price: Some(AccountPriceV1 {
                amount_minor: 500,
                currency: "usd".into(),
            }),
            expires_days: Some(365),
        };
        assert_eq!(
            serde_json::to_value(offer).unwrap(),
            serde_json::json!({
                "kind": "top_up", "id": "small", "name": null, "units": "5000000",
                "price": {"amount_minor": 500, "currency": "usd"},
                "expires_days": 365
            })
        );
        let purchase: AccountPurchaseV1 = serde_json::from_value(
            serde_json::json!({"kind": "plan", "plan": "pro", "interval": "yearly"}),
        )
        .unwrap();
        assert!(matches!(purchase, AccountPurchaseV1::Plan { .. }));
    }

    #[test]
    fn a_server_that_sells_nothing_offers_no_purchase_but_may_host_models() {
        let billing: Billing = serde_json::from_value(serde_json::json!({
            "subscription": {"status": "not_applicable", "paidUntil": null, "cancelAtPeriodEnd": false},
            "paid": false,
            "capabilities": {"hostedAI": true, "synchronization": false},
            "offers": {"subscriptions": [{"plan": "pro", "interval": "monthly"}], "topups": []}
        }))
        .unwrap();
        let usage: Usage = serde_json::from_value(serde_json::json!({
            "cloud_enabled": true, "unit_scale": 1_000_000, "credits_remaining": "2500000",
            "reserved_units": "120000", "bonus_remaining": "1000000",
            "bonus_expires_at": "2026-10-20T00:00:00.000Z", "daily_limit_units": null,
            "allowance": {"total_units": "2000000", "used_units": "300000", "reserved_units": "120000"}
        }))
        .unwrap();
        let view = View {
            server: "http://127.0.0.1:8787".into(),
            custom_server: true,
            config: Some(wire::ClientConfig {
                cloud_enabled: false,
                billing_enabled: true,
                ..Default::default()
            }),
            signed_in: true,
            billing: Some(billing),
            usage: Some(usage),
            ..Default::default()
        };
        let account = project(view);
        assert_eq!(account.billing, Some(false));
        assert!(account.offers.is_empty());
        assert_eq!(account.hosted_models, Some(true));
        let usage = account.usage.unwrap();
        assert_eq!(usage.held.as_deref(), Some("120000"));
        assert_eq!(usage.remaining.as_deref(), Some("2500000"));
        assert_eq!(usage.daily_limit, None);
        assert_eq!(usage.unit_scale, 1_000_000);
    }

    fn staging_usage() -> Usage {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1, "cloud_enabled": true, "policy_revision": 3, "plan": "free",
            "unit_scale": 1_000_000, "credit_units": "10000", "credits_remaining": "1880000",
            "reserved_units": "0", "spent_today": "120000", "bonus_remaining": "0",
            "bonus_expires_at": null, "purchased_remaining": "0", "period_end": null,
            "daily_limit_units": null, "by_model": [], "purchased_balance": null,
            "free_usage_paused": true, "purchased_usage_available": true,
            "allowance": {"total_units": "2000000", "used_units": "120000", "reserved_units": "0",
                          "renews": false, "expires_at": null}
        }))
        .unwrap()
    }

    #[test]
    fn credits_come_from_the_servers_factor_and_a_trial_has_no_renewal() {
        let usage = usage(&staging_usage());
        assert_eq!(usage.credit_units.as_deref(), Some("10000"));
        assert_eq!(usage.allowance_left.as_deref(), Some("1880000"));
        assert_eq!(usage.used_percent, Some(6));
        assert!(usage.one_time && usage.free_paused);
        assert_eq!(usage.purchased_usable, Some(true));
        assert_eq!(usage.resets_at, None);

        // Without the factor there are percentages and no credit numbers.
        let mut older = staging_usage();
        older.credit_units = None;
        assert_eq!(super::usage(&older).credit_units, None);
        assert_eq!(super::usage(&older).used_percent, Some(6));

        // A renewing plan keeps its date; a date on a one-time grant is dropped.
        let mut paid = staging_usage();
        paid.allowance.renews = Some(true);
        paid.period_end = Some("2026-11-01T00:00:00Z".into());
        assert_eq!(
            super::usage(&paid).resets_at.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        paid.allowance.renews = Some(false);
        assert_eq!(super::usage(&paid).resets_at, None);
    }

    #[test]
    fn review_signup_mode_and_plan_change_reach_the_frame() {
        let change: PlanChange = serde_json::from_value(serde_json::json!({
            "id": "7d3c1f0e-5a42-4c8e-9b61-0f2d8a4e6b13", "status": "scheduled",
            "current_plan": "max", "target_plan": "pro", "interval": "monthly",
            "direction": "downgrade", "effective_at": "2026-11-19T00:00:00Z",
            "amount_due_estimate": {"amount_minor": 0, "currency": "USD", "tax_excluded": true},
            "recurring_price": {"amount_minor": 2000, "currency": "USD"},
            "expires_at": "2026-10-20T12:05:00Z", "error": null
        }))
        .unwrap();
        let view = View {
            server: "https://staging-api.zephium.app".into(),
            config: Some(wire::ClientConfig {
                signup_mode: Some(SignupMode::Allowlist),
                billing_enabled: true,
                ..Default::default()
            }),
            signed_in: true,
            billing_review: true,
            plan_change: Some(change),
            ..Default::default()
        };
        let account = project(view);
        assert!(account.billing_review);
        assert_eq!(account.signup_mode, Some(AccountSignupModeV1::Allowlist));
        let change = account.plan_change.unwrap();
        assert_eq!(change.status, "scheduled");
        assert_eq!(change.effective_at.as_deref(), Some("2026-11-19T00:00:00Z"));
        assert_eq!(change.charge.unwrap().amount_minor, 0);
        assert_eq!(change.recurring.unwrap().amount_minor, 2000);
        assert_eq!(change.error, None);
    }

    #[test]
    fn a_session_the_vault_will_not_give_up_reaches_the_frame_still_signed_in() {
        let account = project(View {
            signed_in: true,
            vault_fault: true,
            ..Default::default()
        });
        assert!(account.signed_in && account.vault_fault);
        assert!(!project(View::default()).vault_fault);
        assert!(!unavailable_view().vault_fault);
    }

    #[test]
    fn the_frame_gets_a_picture_revision_never_its_address_or_the_bearer() {
        let url = "https://staging-api.zephium.app/v1/account/avatar/3f9a2c.webp";
        let view = View {
            server: "https://staging-api.zephium.app".into(),
            signed_in: true,
            profile: Some(wire::Profile {
                id: "u1".into(),
                email: "ada@example.com".into(),
                name: None,
                created_at: None,
                avatar_url: Some(url.into()),
            }),
            avatar: Some(7),
            ..Default::default()
        };
        let account = project(view);
        assert_eq!(account.avatar, Some(7));
        let text = serde_json::to_string(&reply_of(account)).unwrap();
        assert!(!text.contains("/v1/account/avatar"), "{text}");
        assert!(!text.to_ascii_lowercase().contains("bearer"), "{text}");
        for (error, fault) in [
            (AccountError::AvatarTimedOut, "avatar_timeout"),
            (AccountError::AvatarTooLarge, "avatar_too_large"),
            (AccountError::AvatarUnsupported, "avatar_unsupported"),
            (AccountError::AvatarDimensions, "avatar_dimensions"),
            (AccountError::AvatarProcessing, "avatar_processing"),
            (AccountError::AvatarInvalid, "avatar_invalid"),
            (AccountError::AvatarCancelled, "avatar_cancelled"),
        ] {
            assert_eq!(
                serde_json::to_value(AccountFaultV1::from(error)).unwrap(),
                fault
            );
        }
        let busy = reply(Err(AccountError::AvatarBusy { retry_after: 9 }));
        assert_eq!(busy.fault, Some(AccountFaultV1::AvatarBusy));
        assert_eq!(busy.retry_after, Some(9));
    }

    fn reply_of(account: AccountV1) -> AccountReplyV1 {
        AccountReplyV1 {
            account,
            fault: None,
            retry_after: None,
        }
    }

    #[test]
    fn funding_shows_only_the_sources_the_contract_names() {
        assert_eq!(funding(Some("mixed")).as_deref(), Some("mixed"));
        assert_eq!(funding(Some("purchased")).as_deref(), Some("purchased"));
        assert_eq!(funding(Some("<script>")), None);
        assert_eq!(funding(None), None);
    }

    #[test]
    fn new_faults_and_sources_have_stable_wire_shapes() {
        for (fault, text) in [
            (AccountFaultV1::BillingReview, "billing_review"),
            (AccountFaultV1::CheckoutInProgress, "checkout_in_progress"),
            (
                AccountFaultV1::SubscriptionChangeRequired,
                "subscription_change_required",
            ),
            (AccountFaultV1::PlanChangeExpired, "plan_change_expired"),
        ] {
            assert_eq!(serde_json::to_value(fault).unwrap(), text);
        }
        assert_eq!(
            AccountFaultV1::from(AccountError::PlanChangeInProgress),
            AccountFaultV1::PlanChangeInProgress
        );
        let task = AccountCallSourceV1::from(CallSource::Task {
            profile: "p".into(),
            work: "w".into(),
        });
        assert_eq!(
            serde_json::to_value(task).unwrap(),
            serde_json::json!({"kind": "task", "profile": "p", "work": "w"})
        );
        assert_eq!(
            serde_json::to_value(AccountCallSourceV1::from(CallSource::Voice)).unwrap(),
            serde_json::json!({"kind": "voice"})
        );
    }
}
