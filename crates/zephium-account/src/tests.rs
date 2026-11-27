use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::vault::{AccountVault, VaultFault};
use crate::wire::Purchase;
use crate::{AccountError, AccountSettings, AccountToken, Service};

#[derive(Default)]
struct MemorySettings(Mutex<HashMap<String, String>>);

impl AccountSettings for MemorySettings {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: String, value: String) -> bool {
        let mut map = self.0.lock().unwrap();
        if value.is_empty() {
            map.remove(&key);
        } else {
            map.insert(key, value);
        }
        true
    }
}

#[derive(Default)]
struct MemoryVault(Mutex<HashMap<String, String>>);

impl MemoryVault {
    fn with(origin: &str, token: &str) -> Arc<Self> {
        let vault = Arc::new(Self::default());
        vault
            .store(origin, &AccountToken::new(token.into()).unwrap())
            .unwrap();
        vault
    }
}

impl AccountVault for MemoryVault {
    fn load(&self, origin: &str) -> Result<AccountToken, VaultFault> {
        let token = self
            .0
            .lock()
            .unwrap()
            .get(origin)
            .cloned()
            .ok_or(VaultFault::Missing)?;
        AccountToken::new(token).map_err(|_| VaultFault::Inaccessible)
    }
    fn store(&self, origin: &str, token: &AccountToken) -> Result<(), VaultFault> {
        self.0
            .lock()
            .unwrap()
            .insert(origin.to_owned(), token.expose().to_owned());
        Ok(())
    }
    fn clear(&self, origin: &str) -> Result<(), VaultFault> {
        self.0.lock().unwrap().remove(origin);
        Ok(())
    }
    fn present(&self, origin: &str) -> Result<bool, VaultFault> {
        Ok(self.0.lock().unwrap().contains_key(origin))
    }
}

#[derive(Clone, Debug)]
struct Seen {
    line: String,
    headers: String,
    body: String,
}

impl Seen {
    fn is(&self, method_path: &str) -> bool {
        self.line.starts_with(&format!("{method_path} "))
    }
}

type Route = Box<dyn Fn(&Seen) -> (u16, String, String) + Send + Sync>;
type Log = Arc<Mutex<Vec<Seen>>>;

/// A loopback account server answering each request with `route`.
async fn server(route: Route) -> (String, Log) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let route = Arc::new(route);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let route = route.clone();
            let log = log.clone();
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0; 4096];
                let (head, body) = loop {
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buffer).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if body.len() >= length {
                            break (head.to_owned(), body.to_owned());
                        }
                    }
                };
                let (line, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
                let seen = Seen {
                    line: line.to_owned(),
                    headers: headers.to_ascii_lowercase(),
                    body,
                };
                log.lock().unwrap().push(seen.clone());
                let (status, extra, body) = route(&seen);
                // Status 0 never answers, like a request still being processed.
                if status == 0 {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    return;
                }
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
    (origin, seen)
}

fn requests(log: &Log, method_path: &str) -> Vec<Seen> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|seen| seen.is(method_path))
        .cloned()
        .collect()
}

fn ok(body: &str) -> (u16, String, String) {
    (200, String::new(), body.to_owned())
}

fn account_routes(seen: &Seen) -> (u16, String, String) {
    let line = seen.line.as_str();
    let signed = seen.headers.contains("authorization: bearer token-1");
    match line.split(' ').take(2).collect::<Vec<_>>()[..] {
        ["POST", "/api/auth/email-otp/send-verification-otp"] => (202, "".into(), r#"{"accepted":true}"#.into()),
        ["POST", "/api/auth/sign-in/email-otp"] if seen.body.contains("\"123456\"") => (
            200,
            "set-auth-token: token-1\r\n".into(),
            r#"{"user":{}}"#.into(),
        ),
        ["POST", "/api/auth/sign-in/email-otp"] => (401, "".into(), r#"{"error":{"code":"invalid_code"}}"#.into()),
        ["GET", "/v1/client-config"] if seen.headers.contains("if-none-match: \"v1\"") => {
            (304, "etag: \"v1\"\r\ncache-control: public, max-age=300, must-revalidate\r\n".into(), String::new())
        }
        ["GET", "/v1/client-config"] => (
            200,
            "etag: \"v1\"\r\ncache-control: public, max-age=0, must-revalidate\r\n".into(),
            r#"{"schema_version":1,"cloud_enabled":true,"signup_open":true,"min_client_version":"2.0.0","billing_enabled":false,"notice":null}"#.into(),
        ),
        [_, path] if path.starts_with("/v1/") && !signed => (401, "".into(), r#"{"error":{"code":"unauthorized"},"request_id":null}"#.into()),
        ["GET", "/v1/account"] => ok(r#"{"id":"u1","email":"a@b.co","name":null,"emailVerified":true,"createdAt":"2026-10-01T00:00:00.000Z"}"#),
        ["PATCH", "/v1/account"] => ok(r#"{"name":"Ada"}"#),
        ["GET", "/v1/billing"] => ok(r#"{"subscription":{"status":"inactive","paidUntil":null,"cancelAtPeriodEnd":false},"paid":false,"capabilities":{"hostedAI":true,"synchronization":false},"offers":{"subscriptions":[{"plan":"pro","interval":"monthly","product_name":"Pro","price":{"amount_minor":900,"currency":"EUR"}}],"topups":[],"unit_scale":1000000}}"#),
        ["GET", "/v1/usage"] => ok(r#"{"plan":"free","unit_scale":1000000,"credits_remaining":"75","reserved_units":"0","allowance":{"total_units":"100","used_units":"25","reserved_units":"0"},"purchased_remaining":"0","bonus_remaining":"0","bonus_expires_at":null,"daily_limit_units":null,"by_model":[]}"#),
        ["POST", "/v1/billing/checkout"] => ok(r#"{"url":"https://sandbox.polar.sh/checkout/x"}"#),
        ["POST", "/v1/account/email/code"] => (401, "".into(), r#"{"error":{"code":"reauthentication_required"},"request_id":null}"#.into()),
        ["PATCH", "/v1/account/email"] => ok(r#"{"updated":true,"reauthentication_required":true}"#),
        ["GET", "/v1/account/export"] => ok(r#"{"schemaVersion":1,"account":{"id":"u1","email":"a@b.co","name":null,"createdAt":"2026-10-01T00:00:00.000Z"},"billing":{},"usage":null,"usage_history_url":"/v1/usage/requests","sessions":[]}"#),
        ["GET", path] if path.starts_with("/v1/usage/requests?cursor=") => ok(r#"{"requests":[],"next_cursor":null}"#),
        ["GET", "/v1/usage/requests"] => ok(r#"{"requests":[{"id":"0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c","model_id":"fast","state":"pending","units":"0","reserved":"1200","started_at":"2026-10-09T10:00:00.000Z","finished_at":null}],"next_cursor":"eyJ0Ijo/MX0+"}"#),
        ["GET", "/v1/usage/requests/0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c"] => ok(r#"{"id":"0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c","state":"settled","model_id":"fast","policy_revision":3,"reserved":"1200","charged":"800","error_class":null,"started_at":"2026-10-09T10:00:00.000Z","finished_at":"2026-10-09T10:00:05.000Z"}"#),
        ["POST", "/v1/logout" | "/v1/logout-all"] => (204, "".into(), String::new()),
        _ => (404, "".into(), r#"{"error":{"code":"not_found"},"request_id":null}"#.into()),
    }
}

fn service(origin: &str, vault: Arc<MemoryVault>, settings: Arc<MemorySettings>) -> Service {
    settings.set("account.server".into(), origin.into());
    Service::new(settings, vault, "Zephium 1.0 · macOS", "1.0.0").unwrap()
}

/// A vault that holds a bearer for the origin but will not hand it over, as a
/// Keychain does when the app's signature no longer matches the item.
struct LockedVault {
    reads: std::sync::atomic::AtomicUsize,
}

impl AccountVault for LockedVault {
    fn load(&self, _: &str) -> Result<AccountToken, VaultFault> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(VaultFault::Inaccessible)
    }
    fn store(&self, _: &str, _: &AccountToken) -> Result<(), VaultFault> {
        Err(VaultFault::Inaccessible)
    }
    fn clear(&self, _: &str) -> Result<(), VaultFault> {
        Ok(())
    }
    fn present(&self, _: &str) -> Result<bool, VaultFault> {
        Ok(true)
    }
}

#[tokio::test]
async fn a_vault_that_will_not_give_up_the_bearer_is_shown_once_and_not_asked_again() {
    let (origin, _seen) = server(Box::new(account_routes)).await;
    let settings = Arc::new(MemorySettings::default());
    settings.set("account.server".into(), origin);
    let vault = Arc::new(LockedVault {
        reads: Default::default(),
    });
    let account = Service::new(settings, vault.clone(), "x", "1.0.0").unwrap();
    let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = changes.clone();
    account.set_observer(Box::new(move || {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let reads = || vault.reads.load(std::sync::atomic::Ordering::SeqCst);

    assert!(account.view().signed_in);
    assert!(!account.view().vault_fault);

    assert_eq!(account.refresh(true).await, Err(AccountError::Vault));
    assert!(account.view().signed_in && account.view().vault_fault);
    assert_eq!(
        (reads(), changes.load(std::sync::atomic::Ordering::SeqCst)),
        (1, 1)
    );

    // Neither another refresh nor a hosted call goes back to the Keychain.
    assert_eq!(account.refresh(true).await, Err(AccountError::Vault));
    assert!(matches!(account.bearer(), Err(AccountError::Vault)));
    assert_eq!(
        (reads(), changes.load(std::sync::atomic::Ordering::SeqCst)),
        (1, 1)
    );

    // Signing in again starts clean, without asking the vault for the old one.
    account.sign_out().await.unwrap();
    let view = account.view();
    assert!(!view.signed_in && !view.vault_fault);
    assert_eq!(reads(), 1);
}

async fn signed_in() -> (Service, Arc<MemoryVault>, Log, String) {
    let (origin, seen) = server(Box::new(account_routes)).await;
    let vault = MemoryVault::with(&origin, "token-1");
    let account = service(&origin, vault.clone(), Arc::new(MemorySettings::default()));
    account.refresh(true).await.unwrap();
    (account, vault, seen, origin)
}

#[tokio::test]
async fn email_code_signs_in_stores_the_bearer_and_loads_the_plan() {
    let (origin, seen) = server(Box::new(account_routes)).await;
    let vault = Arc::new(MemoryVault::default());
    let settings = Arc::new(MemorySettings::default());
    let account = service(&origin, vault.clone(), settings.clone());

    assert!(!account.signed_in());
    account.send_code("  a@b.co ").await.unwrap();
    assert_eq!(account.view().pending_email.as_deref(), Some("a@b.co"));
    assert!(account.view().resend_in.is_some());
    assert!(matches!(
        account.send_code("a@b.co").await,
        Err(AccountError::RateLimited { .. })
    ));

    assert_eq!(
        account.verify("12345").await,
        Err(AccountError::InvalidCode)
    );
    assert_eq!(
        account.verify("000000").await,
        Err(AccountError::InvalidCode)
    );
    account.verify("123456").await.unwrap();

    let view = account.view();
    assert!(view.signed_in);
    assert_eq!(view.pending_email, None);
    assert_eq!(view.profile.unwrap().email, "a@b.co");
    assert_eq!(view.usage.as_ref().unwrap().used_percent(), Some(25));
    assert_eq!(account.plan().as_deref(), Some("free"));
    assert_eq!(vault.load(&origin).unwrap().expose(), "token-1");

    let sign_in = requests(&seen, "POST /api/auth/sign-in/email-otp")
        .into_iter()
        .find(|r| r.body.contains("123456"))
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&sign_in.body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"email": "a@b.co", "otp": "123456", "device": "Zephium 1.0 · macOS"})
    );
    assert!(!sign_in.headers.contains("authorization"));

    // A restart finds the session from the vault and the cached account.
    let again = Service::new(settings.clone(), vault.clone(), "x", "1.0.0").unwrap();
    assert!(again.signed_in());
    assert_eq!(again.view().profile.unwrap().email, "a@b.co");
}

#[tokio::test]
async fn a_refused_bearer_ends_the_session_everywhere() {
    let (origin, _) = server(Box::new(account_routes)).await;
    let vault = MemoryVault::with(&origin, "revoked");
    let settings = Arc::new(MemorySettings::default());
    let account = service(&origin, vault.clone(), settings.clone());
    assert!(account.signed_in());

    assert_eq!(account.refresh(true).await, Err(AccountError::SignedOut));
    assert!(!account.signed_in());
    assert_eq!(vault.present(&origin), Ok(false));
    assert_eq!(settings.get("account.cache"), None);
    assert_eq!(account.bearer().err(), Some(AccountError::SignedOut));
}

#[tokio::test]
async fn bearers_are_scoped_to_the_origin_that_issued_them() {
    let (one, one_seen) = server(Box::new(account_routes)).await;
    let (two, two_seen) = server(Box::new(account_routes)).await;
    let vault = MemoryVault::with(&one, "token-1");
    let settings = Arc::new(MemorySettings::default());
    let account = service(&one, vault.clone(), settings.clone());
    account.refresh(true).await.unwrap();

    account.set_server(Some(&two)).await.unwrap();
    assert!(!account.signed_in());
    assert_eq!(account.bearer().err(), Some(AccountError::SignedOut));
    assert_eq!(account.refresh(true).await, Ok(()));
    assert!(two_seen.lock().unwrap().is_empty());
    // Nothing was revoked on the first server, and its session waits there.
    assert!(requests(&one_seen, "POST /v1/logout").is_empty());
    assert_eq!(vault.load(&one).unwrap().expose(), "token-1");

    account.set_server(Some(&one)).await.unwrap();
    assert!(account.signed_in());
    account.refresh(true).await.unwrap();
    assert_eq!(account.view().profile.unwrap().email, "a@b.co");
}

#[tokio::test]
async fn client_config_is_revalidated_by_etag_and_flags_old_builds() {
    let (origin, seen) = server(Box::new(account_routes)).await;
    let account = service(
        &origin,
        Arc::new(MemoryVault::default()),
        Arc::new(MemorySettings::default()),
    );
    let config = account.load_config().await.unwrap();
    assert!(config.cloud_enabled);
    assert!(account.view().update_required);
    // max-age=0 on the first answer: the next read revalidates and gets a 304.
    let again = account.load_config().await.unwrap();
    assert_eq!(again, config);
    let reads = requests(&seen, "GET /v1/client-config");
    assert_eq!(reads.len(), 2);
    assert!(!reads[0].headers.contains("if-none-match"));
    assert!(reads[1].headers.contains("if-none-match: \"v1\""));
    // The 304 allowed five minutes, so a third read stays local.
    account.load_config().await.unwrap();
    assert_eq!(requests(&seen, "GET /v1/client-config").len(), 2);
}

#[tokio::test]
async fn checkout_opens_only_on_the_billing_provider_and_is_watched() {
    let (account, _, seen, _) = signed_in().await;

    let url = account
        .checkout(&Purchase::Plan {
            plan: "pro".into(),
            interval: "monthly".into(),
        })
        .await
        .unwrap();
    assert_eq!(url.host_str(), Some("sandbox.polar.sh"));
    assert!(account.checkout_open());
    let body = requests(&seen, "POST /v1/billing/checkout")[0].body.clone();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({"plan": "pro", "interval": "monthly"})
    );

    // Without the on-demand refresh route, billing falls back to a plain read.
    let billing = account.refresh_billing().await.unwrap();
    assert!(!billing.paid);
    assert!(billing.sells());
}

#[tokio::test]
async fn billing_refresh_stays_within_six_a_minute() {
    let (account, _, seen, _) = signed_in().await;
    for _ in 0..6 {
        account.refresh_billing().await.unwrap();
    }
    assert!(matches!(
        account.refresh_billing().await,
        Err(AccountError::RateLimited { retry_after }) if retry_after <= 60
    ));
    assert_eq!(requests(&seen, "POST /v1/billing/refresh").len(), 6);
}

#[tokio::test]
async fn an_email_change_on_an_old_session_asks_to_sign_in_again_without_signing_out() {
    let (account, vault, seen, origin) = signed_in().await;
    assert_eq!(
        account.send_email_change_code("a@b.co").await,
        Err(AccountError::Invalid)
    );
    assert_eq!(
        account.send_email_change_code("new@b.co").await,
        Err(AccountError::ReauthRequired)
    );
    assert!(account.signed_in());
    assert!(vault.present(&origin).unwrap());

    // Signing in again replaces the old session and revokes it on the server.
    account.send_reauth_code().await.unwrap();
    assert_eq!(account.view().pending_email.as_deref(), Some("a@b.co"));
    account.verify("123456").await.unwrap();
    assert_eq!(requests(&seen, "POST /v1/logout").len(), 1);
    assert!(account.signed_in());
}

#[tokio::test]
async fn a_completed_email_change_ends_the_session_here() {
    let (account, vault, seen, origin) = signed_in().await;
    assert_eq!(
        account.change_email("new@b.co", "12").await,
        Err(AccountError::InvalidCode)
    );
    account.change_email(" new@b.co ", "654321").await.unwrap();
    assert!(!account.signed_in());
    assert_eq!(vault.present(&origin), Ok(false));
    let body = requests(&seen, "PATCH /v1/account/email")[0].body.clone();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({"email": "new@b.co", "otp": "654321"})
    );
}

#[tokio::test]
async fn name_export_history_and_receipts_follow_the_contract() {
    let (account, _, seen, _) = signed_in().await;

    account.set_name(Some("  Ada ")).await.unwrap();
    assert_eq!(account.view().profile.unwrap().name.as_deref(), Some("Ada"));
    let body = requests(&seen, "PATCH /v1/account")[0].body.clone();
    assert_eq!(body, r#"{"name":"Ada"}"#);
    account.set_name(None).await.unwrap();
    assert_eq!(
        requests(&seen, "PATCH /v1/account")[1].body,
        r#"{"name":null}"#
    );
    assert_eq!(
        account.set_name(Some(&"x".repeat(101))).await,
        Err(AccountError::Invalid)
    );

    let export = account.export().await.unwrap();
    assert_eq!(export["schemaVersion"], 1);

    let page = account.usage_history(None).await.unwrap();
    assert_eq!(page.requests[0].state, "pending");
    let cursor = page.next_cursor.unwrap();
    account.usage_history(Some(&cursor)).await.unwrap();
    // The opaque cursor goes back exactly, percent-encoded only as a query.
    let next = &requests(&seen, "GET /v1/usage/requests?cursor=eyJ0Ijo%2FMX0%2B")[0];
    assert!(next.line.contains("cursor=eyJ0Ijo%2FMX0%2B"));
    assert_eq!(
        account.usage_history(Some(&"c".repeat(201))).await.err(),
        Some(AccountError::Invalid)
    );

    let receipt = account
        .confirm_request("0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c")
        .await
        .unwrap();
    assert!(receipt.settled());
    assert_eq!(receipt.charged.as_deref(), Some("800"));
    assert_eq!(account.view().unsettled.len(), 1);
    assert_eq!(
        account
            .receipt("0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6d")
            .await,
        Err(AccountError::NotFound)
    );
    assert!(account.signed_in());
}

#[tokio::test]
async fn signing_out_everywhere_needs_the_servers_word() {
    let vault = MemoryVault::with("http://127.0.0.1:9", "token-1");
    let account = service(
        "http://127.0.0.1:9",
        vault.clone(),
        Arc::new(MemorySettings::default()),
    );
    assert_eq!(
        account.sign_out_everywhere().await,
        Err(AccountError::Offline)
    );
    assert!(account.signed_in());

    let (account, vault, seen, origin) = signed_in().await;
    account.sign_out_everywhere().await.unwrap();
    assert!(!account.signed_in());
    assert_eq!(vault.present(&origin), Ok(false));
    assert_eq!(requests(&seen, "POST /v1/logout-all").len(), 1);
}

#[tokio::test]
async fn sign_out_clears_locally_even_when_the_server_is_gone() {
    let vault = MemoryVault::with("http://127.0.0.1:9", "token-1");
    let settings = Arc::new(MemorySettings::default());
    let account = service("http://127.0.0.1:9", vault.clone(), settings);
    account.sign_out().await.unwrap();
    assert!(!account.signed_in());
    assert_eq!(vault.present("http://127.0.0.1:9"), Ok(false));
}

#[tokio::test]
async fn changing_server_remembers_only_custom_servers() {
    let vault = MemoryVault::with("http://127.0.0.1:9", "token-1");
    let settings = Arc::new(MemorySettings::default());
    let account = service("http://127.0.0.1:9", vault.clone(), settings.clone());

    assert_eq!(
        account.set_server(Some("http://example.com")).await,
        Err(AccountError::Invalid)
    );
    assert!(account.signed_in());

    account.set_server(None).await.unwrap();
    assert!(!account.signed_in());
    assert_eq!(account.origin(), crate::DEFAULT_ORIGIN);
    assert_eq!(settings.get("account.server"), None);
    assert!(!account.view().custom_server);
    assert!(vault.present("http://127.0.0.1:9").unwrap());
}

const QUOTE: &str = "7d3c1f0e-5a42-4c8e-9b61-0f2d8a4e6b13";

fn change(status: &str) -> String {
    format!(
        r#"{{"id":"{QUOTE}","status":"{status}","current_plan":"pro","target_plan":"max","interval":"monthly","direction":"upgrade","effective_at":null,"amount_due_estimate":{{"amount_minor":6500,"currency":"USD","tax_excluded":true}},"recurring_price":{{"amount_minor":10000,"currency":"USD"}},"expires_at":"2026-10-20T12:05:00.000Z","error":null}}"#
    )
}

#[derive(Default)]
struct Billing {
    fresh: bool,
    polls: usize,
    expire: bool,
    review: bool,
    checkouts: usize,
}

fn refusal(status: u16, code: &str) -> (u16, String, String) {
    (
        status,
        String::new(),
        format!(r#"{{"error":{{"code":"{code}"}},"request_id":null}}"#),
    )
}

/// The account routes plus the Pro/Max change contract. Confirmation needs a
/// sign-in since the server was started, as the real one needs a fresh session.
fn billing_server(state: Arc<Mutex<Billing>>) -> Route {
    Box::new(move |seen| {
        let mut state = state.lock().unwrap();
        let line = seen.line.split(' ').take(2).collect::<Vec<_>>().join(" ");
        match line.as_str() {
            "POST /api/auth/sign-in/email-otp" if seen.body.contains("\"123456\"") => {
                state.fresh = true;
                account_routes(seen)
            }
            "POST /v1/billing/subscription/preview" => ok(&change("quoted")),
            "POST /v1/billing/subscription/change" if state.expire => {
                refusal(409, "plan_change_expired")
            }
            "POST /v1/billing/subscription/change" if !state.fresh => {
                refusal(401, "reauthentication_required")
            }
            "POST /v1/billing/subscription/change" => (202, String::new(), change("queued")),
            "GET /v1/usage" if state.review => refusal(403, "billing_review_required"),
            "POST /v1/billing/checkout" => {
                state.checkouts += 1;
                if state.review {
                    return refusal(403, "billing_review_required");
                }
                if state.checkouts > 1 {
                    return refusal(409, "checkout_in_progress");
                }
                ok(r#"{"url":"https://sandbox.polar.sh/checkout/first"}"#)
            }
            "PUT /v1/usage/limits" => (204, String::new(), String::new()),
            path if path == format!("GET /v1/billing/subscription/changes/{QUOTE}") => {
                state.polls += 1;
                let status = match state.polls {
                    1 => "executing",
                    2 => "uncertain",
                    _ => "applied",
                };
                ok(&change(status))
            }
            _ => account_routes(seen),
        }
    })
}

type Held = (
    Service,
    Arc<Mutex<Billing>>,
    Log,
    Arc<MemorySettings>,
    Arc<MemoryVault>,
);

async fn billing_account() -> Held {
    let state = Arc::new(Mutex::new(Billing::default()));
    let (origin, seen) = server(billing_server(state.clone())).await;
    let vault = MemoryVault::with(&origin, "token-1");
    let settings = Arc::new(MemorySettings::default());
    let account = service(&origin, vault.clone(), settings.clone());
    account.refresh(true).await.unwrap();
    (account, state, seen, settings, vault)
}

#[tokio::test]
async fn a_plan_change_is_confirmed_after_a_fresh_sign_in_and_followed_to_its_end() {
    let (account, _, seen, settings, vault) = billing_account().await;

    let quote = account.preview_plan_change("max", "monthly").await.unwrap();
    assert_eq!(quote.status, "quoted");
    assert_eq!(
        quote.amount_due_estimate.as_ref().unwrap().amount_minor,
        6500
    );
    let preview = &requests(&seen, "POST /v1/billing/subscription/preview")[0];
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&preview.body).unwrap(),
        serde_json::json!({"plan": "max", "interval": "monthly"})
    );
    assert_eq!(account.view().plan_change.unwrap().id, QUOTE);

    // An old session cannot confirm; the quote waits and the session stays.
    assert_eq!(
        account.confirm_plan_change().await,
        Err(AccountError::ReauthRequired)
    );
    assert!(account.signed_in());
    assert_eq!(account.view().plan_change.unwrap().status, "quoted");

    account.send_reauth_code().await.unwrap();
    account.verify("123456").await.unwrap();
    let held = account
        .view()
        .plan_change
        .expect("the quote survives signing in again");
    assert_eq!(held.id, QUOTE);

    let started = account.confirm_plan_change().await.unwrap();
    assert_eq!(started.status, "queued");
    assert!(account.plan_change_in_flight());
    let confirm = requests(&seen, "POST /v1/billing/subscription/change");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&confirm.last().unwrap().body).unwrap(),
        serde_json::json!({"quote_id": QUOTE})
    );

    // Another quote is refused while this one is carried out.
    assert_eq!(
        account.preview_plan_change("pro", "monthly").await.err(),
        Some(AccountError::PlanChangeInProgress)
    );
    // Closing the app does not lose it.
    let again = Service::new(settings.clone(), vault.clone(), "x", "1.0.0").unwrap();
    assert_eq!(again.view().plan_change.unwrap().status, "queued");
    assert!(again.plan_change_open());

    // It cannot be put away while in flight; `uncertain` is only waited on.
    account.dismiss_plan_change();
    assert!(account.view().plan_change.is_some());
    let next = account.refresh_plan_change().await.unwrap().unwrap();
    assert_eq!(next.status, "executing");
    let next = account.refresh_plan_change().await.unwrap().unwrap();
    assert_eq!(next.status, "uncertain");
    assert!(account.plan_change_in_flight());
    assert_eq!(
        requests(&seen, "POST /v1/billing/subscription/change").len(),
        confirm.len()
    );

    let done = account.refresh_plan_change().await.unwrap().unwrap();
    assert_eq!(done.status, "applied");
    assert!(!account.plan_change_in_flight());
    // Finishing reads billing and usage again.
    assert!(!requests(&seen, "POST /v1/billing/refresh").is_empty());
    account.dismiss_plan_change();
    assert!(account.view().plan_change.is_none());
    assert_eq!(settings.get("account.plan_change"), None);
}

#[tokio::test]
async fn a_held_change_belongs_to_one_account_on_one_server() {
    let (account, _, _, settings, vault) = billing_account().await;
    account.preview_plan_change("max", "yearly").await.unwrap();
    let held = settings.get("account.plan_change").unwrap();
    let cache = settings.get("account.cache").unwrap();
    assert!(held.contains(&account.origin()));

    // Another server never sees it, and another account's session hides it.
    let other = Service::new(settings.clone(), vault.clone(), "x", "1.0.0").unwrap();
    other.set_server(Some("http://127.0.0.1:9")).await.unwrap();
    assert!(other.view().plan_change.is_none());
    let stranger = Arc::new(MemorySettings::default());
    stranger.set(
        "account.plan_change".into(),
        held.replace("\"u1\"", "\"someone-else\""),
    );
    stranger.set("account.server".into(), account.origin());
    stranger.set("account.cache".into(), cache);
    let stranger = Service::new(stranger, vault, "x", "1.0.0").unwrap();
    assert!(stranger.view().plan_change.is_none());
}

#[tokio::test]
async fn an_expired_quote_is_dropped_so_a_new_one_must_be_asked_for() {
    let (account, state, _, settings, _) = billing_account().await;
    account.preview_plan_change("max", "monthly").await.unwrap();
    state.lock().unwrap().expire = true;
    assert_eq!(
        account.confirm_plan_change().await.err(),
        Some(AccountError::PlanChangeExpired)
    );
    assert!(account.view().plan_change.is_none());
    assert_eq!(settings.get("account.plan_change"), None);
}

#[tokio::test]
async fn plan_ids_and_intervals_are_checked_before_anything_is_sent() {
    let (account, _, seen, _, _) = billing_account().await;
    for (plan, interval) in [
        ("", "monthly"),
        ("pro max", "monthly"),
        ("max", "weekly"),
        ("a/b", "yearly"),
    ] {
        assert_eq!(
            account.preview_plan_change(plan, interval).await.err(),
            Some(AccountError::Invalid),
            "{plan} {interval}"
        );
    }
    assert!(requests(&seen, "POST /v1/billing/subscription/preview").is_empty());
}

#[tokio::test]
async fn a_checkout_link_is_kept_to_retry_the_same_checkout() {
    let (account, state, _, _, _) = billing_account().await;
    let pro = Purchase::Plan {
        plan: "pro".into(),
        interval: "monthly".into(),
    };
    let first = account.checkout(&pro).await.unwrap();
    // The service now refuses a second subscription checkout; the same link comes back.
    let retry = account.checkout(&pro).await.unwrap();
    assert_eq!(first, retry);
    assert_eq!(state.lock().unwrap().checkouts, 1);
    // A different purchase is not answered with that link.
    let max = Purchase::Plan {
        plan: "max".into(),
        interval: "monthly".into(),
    };
    assert_eq!(
        account.checkout(&max).await.err(),
        Some(AccountError::CheckoutInProgress)
    );
}

#[tokio::test]
async fn billing_review_is_read_from_usage_and_blocks_a_new_checkout() {
    let (account, state, _, _, _) = billing_account().await;
    assert!(!account.view().billing_review);
    state.lock().unwrap().review = true;
    account.refresh(true).await.unwrap();
    let view = account.view();
    assert!(view.billing_review);
    // The last known usage stays for the page to show.
    assert!(view.usage.is_some());
    let pro = Purchase::Plan {
        plan: "pro".into(),
        interval: "monthly".into(),
    };
    assert_eq!(
        account.checkout(&pro).await.err(),
        Some(AccountError::BillingReview)
    );
    state.lock().unwrap().review = false;
    account.refresh(true).await.unwrap();
    assert!(!account.view().billing_review);
}

#[tokio::test]
async fn the_daily_limit_is_set_in_units_or_removed() {
    let (account, _, seen, _, _) = billing_account().await;
    account.set_daily_limit(Some("500000")).await.unwrap();
    account.set_daily_limit(None).await.unwrap();
    let puts = requests(&seen, "PUT /v1/usage/limits");
    assert_eq!(puts[0].body, r#"{"daily_limit_units":"500000"}"#);
    assert_eq!(puts[1].body, r#"{"daily_limit_units":null}"#);
    for refused in ["-1", "1.5", "", "1e6", "  "] {
        assert_eq!(
            account.set_daily_limit(Some(refused)).await.err(),
            Some(AccountError::Invalid),
            "{refused:?}"
        );
    }
    assert_eq!(requests(&seen, "PUT /v1/usage/limits").len(), 2);
}

#[tokio::test]
async fn which_task_made_a_call_is_kept_here_and_goes_with_signing_out() {
    use crate::CallSource;
    let (account, _, _, settings, vault) = billing_account().await;
    let task = CallSource::Task {
        profile: "p1".into(),
        work: "w1".into(),
    };
    account.record_call_source("req-1", task.clone());
    account.record_call_source("req-2", CallSource::Voice);
    assert_eq!(
        account.call_sources(&["req-1".into(), "req-2".into(), "req-3".into()]),
        vec![Some(task.clone()), Some(CallSource::Voice), None]
    );
    let again = Service::new(settings.clone(), vault, "x", "1.0.0").unwrap();
    assert_eq!(again.call_sources(&["req-1".into()]), vec![Some(task)]);

    account.sign_out().await.unwrap();
    assert_eq!(account.call_sources(&["req-1".into()]), vec![None]);
    assert_eq!(settings.get("account.call_tasks"), None);
}

/// An account server whose account names a picture at `picture` (a path on
/// `host`, or on this server when `host` is empty), served as fake WebP bytes
/// tagged by the picture's name.
fn avatar_server(host: Arc<Mutex<String>>, picture: Arc<Mutex<Option<String>>>) -> Route {
    Box::new(move |seen: &Seen| {
        let signed = seen.headers.contains("authorization: bearer token-1");
        let line = seen.line.clone();
        let parts: Vec<&str> = line.split(' ').take(2).collect();
        match parts[..] {
            [_, path] if path.starts_with("/v1/") && !signed => refusal(401, "unauthorized"),
            ["GET", "/v1/account"] => {
                let url = picture
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|path| format!("\"{}{path}\"", host.lock().unwrap()))
                    .unwrap_or_else(|| "null".into());
                ok(&format!(
                    r#"{{"id":"u1","email":"a@b.co","name":null,"avatar_url":{url}}}"#
                ))
            }
            ["GET", "/v1/account/avatar/gone.webp"] => refusal(404, "avatar_not_found"),
            ["GET", path] if path.starts_with("/v1/account/avatar/") => {
                let name = path.trim_start_matches("/v1/account/avatar/");
                let etag = format!("\"{name}\"");
                if seen.headers.contains(&format!("if-none-match: {etag}")) {
                    (304, format!("etag: {etag}\r\n"), String::new())
                } else {
                    (
                        200,
                        format!("etag: {etag}\r\n"),
                        format!("RIFF\0\0\0\0WEBPVP8 {name}"),
                    )
                }
            }
            ["PUT", "/v1/account/avatar"] if seen.body.contains("slow") => {
                (0, String::new(), String::new())
            }
            ["PUT", "/v1/account/avatar"] => {
                *picture.lock().unwrap() = Some("/v1/account/avatar/v2.webp".into());
                ok(&format!(
                    r#"{{"avatar_url":"{}/v1/account/avatar/v2.webp"}}"#,
                    host.lock().unwrap()
                ))
            }
            ["DELETE", "/v1/account/avatar"] => {
                *picture.lock().unwrap() = None;
                ok(r#"{"avatar_url":null}"#)
            }
            _ => account_routes(seen),
        }
    })
}

struct Pictured {
    account: Arc<Service>,
    seen: Log,
    picture: Arc<Mutex<Option<String>>>,
}

async fn pictured(picture: Option<&str>, foreign: Option<&str>) -> Pictured {
    let host = Arc::new(Mutex::new(String::new()));
    let named = Arc::new(Mutex::new(picture.map(str::to_owned)));
    let (origin, seen) = server(avatar_server(host.clone(), named.clone())).await;
    *host.lock().unwrap() = foreign.map_or_else(|| origin.clone(), str::to_owned);
    let vault = MemoryVault::with(&origin, "token-1");
    let account = Arc::new(service(&origin, vault, Arc::new(MemorySettings::default())));
    account.refresh(true).await.unwrap();
    Pictured {
        account,
        seen,
        picture: named,
    }
}

fn png(extra: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(8 + extra, b'p');
    bytes
}

#[tokio::test]
async fn the_picture_is_fetched_with_the_bearer_revalidated_by_etag_and_gone_on_sign_out() {
    let held = pictured(Some("/v1/account/avatar/v1.webp"), None).await;
    let account = &held.account;
    let revision = account.view().avatar.expect("picture held");
    let bytes = account.avatar_image(revision).unwrap();
    assert!(bytes.ends_with(b"v1.webp"));
    let fetches = requests(&held.seen, "GET /v1/account/avatar/v1.webp");
    assert_eq!(fetches.len(), 1);
    assert!(fetches[0].headers.contains("authorization: bearer token-1"));
    assert!(!fetches[0].headers.contains("if-none-match"));

    // Shown again: a conditional request, and the same bytes stay.
    account.load_avatar().await.unwrap();
    let fetches = requests(&held.seen, "GET /v1/account/avatar/v1.webp");
    assert_eq!(fetches.len(), 2);
    assert!(fetches[1].headers.contains("if-none-match: \"v1.webp\""));
    assert_eq!(account.view().avatar, Some(revision));

    // Another revision is never served, and nothing is after signing out.
    assert_eq!(account.avatar_image(revision + 1), None);
    account.sign_out().await.unwrap();
    assert_eq!(account.view().avatar, None);
    assert_eq!(account.avatar_image(revision), None);
}

#[tokio::test]
async fn a_picture_named_on_another_origin_never_gets_the_bearer() {
    let (elsewhere, elsewhere_seen) = server(Box::new(account_routes)).await;
    let held = pictured(Some("/v1/account/avatar/v1.webp"), Some(&elsewhere)).await;
    assert_eq!(held.account.view().avatar, None);
    assert_eq!(
        held.account.load_avatar().await,
        Err(AccountError::Protocol)
    );
    assert!(elsewhere_seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_missing_picture_reads_the_account_again_and_falls_back_to_initials() {
    let held = pictured(Some("/v1/account/avatar/gone.webp"), None).await;
    assert_eq!(held.account.view().avatar, None);
    held.account.load_avatar().await.unwrap();
    // Asked once, the account read again once, and not asked again.
    assert_eq!(
        requests(&held.seen, "GET /v1/account/avatar/gone.webp").len(),
        1
    );
    assert_eq!(requests(&held.seen, "GET /v1/account").len(), 2);
}

#[tokio::test]
async fn uploads_are_checked_before_sending_and_replace_the_picture() {
    let held = pictured(Some("/v1/account/avatar/v1.webp"), None).await;
    let account = &held.account;
    let before = account.view().avatar.unwrap();

    let oversized = png(crate::MAX_AVATAR_UPLOAD_BYTES);
    for (bytes, refused) in [
        (Vec::new(), AccountError::AvatarInvalid),
        (oversized, AccountError::AvatarTooLarge),
        (b"GIF89a....".to_vec(), AccountError::AvatarUnsupported),
        (b"<svg xmlns=".to_vec(), AccountError::AvatarUnsupported),
    ] {
        assert_eq!(account.upload_avatar(&bytes).await, Err(refused));
    }
    assert!(requests(&held.seen, "PUT /v1/account/avatar").is_empty());

    account.upload_avatar(&png(64)).await.unwrap();
    let put = &requests(&held.seen, "PUT /v1/account/avatar")[0];
    assert!(put
        .headers
        .contains("content-type: multipart/form-data; boundary=zephium-avatar-"));
    assert!(put
        .body
        .contains("name=\"file\"; filename=\"avatar.png\"\r\nContent-Type: image/png"));
    let after = account.view().avatar.expect("new picture held");
    assert_ne!(after, before);
    assert_eq!(account.avatar_image(before), None);
    assert!(account.avatar_image(after).unwrap().ends_with(b"v2.webp"));

    account.remove_avatar().await.unwrap();
    assert_eq!(account.view().avatar, None);
    assert_eq!(account.view().profile.unwrap().avatar_url, None);
    assert_eq!(*held.picture.lock().unwrap(), None);
}

#[tokio::test]
async fn removing_cancels_an_upload_under_way_before_it_is_sent() {
    let held = pictured(None, None).await;
    let mut slow = png(16);
    slow.extend_from_slice(b"slow");
    let uploading = {
        let account = held.account.clone();
        tokio::spawn(async move { account.upload_avatar(&slow).await })
    };
    for _ in 0..200 {
        if !requests(&held.seen, "PUT /v1/account/avatar").is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    held.account.remove_avatar().await.unwrap();
    assert_eq!(uploading.await.unwrap(), Err(AccountError::AvatarCancelled));
    let order: Vec<String> = held
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|seen| seen.line.clone())
        .filter(|line| line.contains("/v1/account/avatar"))
        .collect();
    assert!(order[0].starts_with("PUT ") && order[1].starts_with("DELETE "));
    assert_eq!(held.account.view().avatar, None);
}
