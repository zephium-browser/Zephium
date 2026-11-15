//! CookieManager operations are URL-scoped, serialized and reauthorized at settlement.
use super::*;
use serde_json::{json, Value};
use webkit2gtk::{CookieManagerExt, WebContextExt, WebViewExt, WebsiteDataManagerExt};
use zephium_webext_runtime::api::cookies::{Operation, Request};
use zephium_webext_runtime::{context::Sender, dispatch::Responder, ApiError};

struct Job {
    sender: Box<Sender>,
    revision: u64,
    request: Request,
    responder: Responder,
}

#[derive(Default)]
pub(super) struct Backend {
    queue: RefCell<VecDeque<Job>>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
    failed: Cell<bool>,
}

impl Backend {
    pub(super) fn retire(&self) {
        if let Some(task) = self.task.borrow_mut().take() {
            task.abort();
        }
        self.queue.borrow_mut().clear();
        self.failed.set(true);
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.retire();
    }
}

fn error(message: &'static str) -> ApiError {
    ApiError::other(message)
}
fn allowed(inner: &Inner, sender: &Sender, url: Option<&url::Url>, revision: u64) -> bool {
    !inner.retired.get()
        && inner
            .bridge
            .with_runtime(|host, _| {
                host.cookies_allowed(sender, url) && host.cookie_revision(sender) == Some(revision)
            })
            .unwrap_or(false)
}

pub(super) fn submit(
    inner: &Rc<Inner>,
    sender: Box<Sender>,
    revision: u64,
    request: Request,
    responder: Responder,
) {
    if inner.cookies.failed.get()
        || inner.cookies.queue.borrow().len() >= 64
        || !allowed(inner, &sender, request.url.as_ref(), revision)
    {
        let _ = inner.bridge.with_runtime(|host, out| {
            host.settle_call(
                responder,
                Err(error("Cookie access is unavailable or busy.")),
                out,
            )
        });
        return;
    }
    inner.cookies.queue.borrow_mut().push_back(Job {
        revision,
        sender,
        request,
        responder,
    });
    if inner.cookies.task.borrow().is_some() {
        return;
    }
    let weak = Rc::downgrade(inner);
    let task = glib::MainContext::default().spawn_local(async move {
        loop {
            let Some(inner) = weak.upgrade().filter(|inner| !inner.retired.get()) else {
                return;
            };
            let Some(job) = inner.cookies.queue.borrow_mut().pop_front() else {
                inner.cookies.task.borrow_mut().take();
                return;
            };
            let result = if inner.cookies.failed.get()
                || !allowed(&inner, &job.sender, job.request.url.as_ref(), job.revision)
            {
                Err(ApiError::context_invalidated())
            } else {
                futures_lite::future::race(
                    perform(&inner, &job.sender, &job.request, job.revision),
                    async {
                        async_io::Timer::after(Duration::from_secs(10)).await;
                        inner.cookies.failed.set(true);
                        Err(error("The native cookie operation timed out."))
                    },
                )
                .await
            };
            let result = if allowed(&inner, &job.sender, job.request.url.as_ref(), job.revision) {
                result
            } else {
                Err(ApiError::context_invalidated())
            };
            let _ = inner
                .bridge
                .with_runtime(|host, out| host.settle_call(job.responder, result, out));
            inner.schedule();
        }
    });
    *inner.cookies.task.borrow_mut() = Some(task);
}

fn project(cookie: &mut soup::Cookie) -> Result<Value, ApiError> {
    let domain = cookie
        .domain()
        .filter(|domain| !domain.is_empty())
        .ok_or_else(|| error("Native cookie has no domain."))?;
    let name = cookie
        .name()
        .ok_or_else(|| error("Native cookie has no name."))?;
    let value = cookie
        .value()
        .ok_or_else(|| error("Native cookie has no value."))?;
    let path = cookie
        .path()
        .ok_or_else(|| error("Native cookie has no path."))?;
    let expires = cookie.expires();
    let mut result = json!({"name":name.as_str(), "value":value.as_str(), "domain":domain.as_str(), "hostOnly":!domain.starts_with('.'), "path":path.as_str(), "secure":cookie.is_secure(), "httpOnly":cookie.is_http_only(), "session":expires.is_none(), "storeId":"0"});
    if let Some(expires) = expires {
        result["expirationDate"] = json!(expires.to_unix());
    }
    match cookie.same_site_policy() {
        soup::SameSitePolicy::None => result["sameSite"] = json!("no_restriction"),
        soup::SameSitePolicy::Strict => result["sameSite"] = json!("strict"),
        // Soup loses the distinction between explicit Lax and an omitted attribute.
        soup::SameSitePolicy::Lax => {}
        _ => return Err(error("Native cookie SameSite state is unavailable.")),
    }
    Ok(result)
}

async fn read(
    manager: &webkit2gtk::CookieManager,
    url: &url::Url,
) -> Result<Vec<(soup::Cookie, Value)>, ApiError> {
    let cookies = manager
        .cookies_future(url.as_str())
        .await
        .map_err(|_| error("Native cookies could not be read."))?;
    if cookies.len() > 4096 {
        return Err(error("The URL cookie enumeration exceeds its limit."));
    }
    let mut bytes = 0;
    let mut rows = Vec::with_capacity(cookies.len());
    for mut cookie in cookies {
        let value = project(&mut cookie)?;
        bytes += value.to_string().len();
        if bytes > 1024 * 1024 {
            return Err(error("The URL cookie enumeration exceeds its byte limit."));
        }
        rows.push((cookie, value));
    }
    Ok(rows)
}

fn selected(
    rows: Vec<(soup::Cookie, Value)>,
    name: &str,
) -> Result<Option<(soup::Cookie, Value)>, ApiError> {
    let mut rows: Vec<_> = rows
        .into_iter()
        .filter(|(_, row)| row["name"] == name)
        .collect();
    rows.sort_by_key(|(_, row)| std::cmp::Reverse(row["path"].as_str().unwrap_or("").len()));
    if rows.len() > 1
        && rows[0].1["path"].as_str().map(str::len) == rows[1].1["path"].as_str().map(str::len)
    {
        return Err(error(
            "This engine cannot order cookies with equal paths by creation time.",
        ));
    }
    Ok(rows.into_iter().next())
}

fn matches(row: &Value, details: &Value) -> bool {
    ["name", "path", "secure", "session"]
        .iter()
        .all(|field| details.get(field).is_none_or(|value| value == &row[field]))
        && details["domain"].as_str().is_none_or(|domain| {
            let domain = domain.strip_prefix('.').unwrap_or(domain);
            let found = row["domain"].as_str().unwrap_or("").trim_start_matches('.');
            found == domain
                || found
                    .strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
}

fn make(request: &Request) -> Result<soup::Cookie, ApiError> {
    let details = &request.details;
    let url = request
        .url
        .as_ref()
        .ok_or_else(|| error("Cookie URL is required."))?;
    let name = details["name"].as_str().unwrap_or("");
    let value = details["value"].as_str().unwrap_or("");
    if name.is_empty()
        || name.len() + value.len() > 4096
        || name
            .bytes()
            .any(|c| c <= 0x20 || c >= 0x7f || b"()<>@,;:\\\"/[]?={}".contains(&c))
        || value.bytes().any(|c| c < 0x20 || c == 0x7f || c == b';')
    {
        return Err(error("Invalid cookie name or value."));
    }
    let host = url.host_str().ok_or_else(|| error("Invalid cookie URL."))?;
    if host.starts_with('.')
        || host.ends_with('.')
        || matches!(url.host(), Some(url::Host::Ipv6(_)))
    {
        return Err(error("This cookie host cannot be verified by the engine."));
    }
    let domain = if let Some(domain) = details["domain"].as_str() {
        if domain.contains('%') {
            return Err(error("Invalid cookie domain."));
        }
        let raw = domain.strip_prefix('.').unwrap_or(domain);
        let parsed = url::Host::parse(raw).map_err(|_| error("Invalid cookie domain."))?;
        let canonical = parsed.to_string();
        if !matches!(parsed, url::Host::Domain(_))
            || canonical.ends_with('.')
            || soup::tld_domain_is_public_suffix(&canonical)
            || !(host == canonical
                || host
                    .strip_suffix(&canonical)
                    .is_some_and(|prefix| prefix.ends_with('.')))
        {
            return Err(error(
                "Cookie domain is outside the URL host or is a public suffix.",
            ));
        }
        format!(".{canonical}")
    } else {
        host.to_owned()
    };
    let default_path = url
        .path()
        .rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or("/");
    let path = details["path"].as_str().unwrap_or(default_path);
    if !path.starts_with('/')
        || path.len() > 1024
        || path.bytes().any(|c| c < 0x20 || c == 0x7f || c == b';')
    {
        return Err(error("Invalid cookie path."));
    }
    let mut probe_url = url.clone();
    probe_url.set_path(path);
    if probe_url.path() != path {
        return Err(error("The cookie path cannot be verified by this engine."));
    }
    let secure = details["secure"].as_bool().unwrap_or(false);
    let http_only = details["httpOnly"].as_bool().unwrap_or(false);
    let prefix_name = name.to_ascii_lowercase();
    if ((prefix_name.starts_with("__secure-")
        || prefix_name.starts_with("__host-")
        || prefix_name.starts_with("__http-"))
        && !secure)
        || (prefix_name.starts_with("__host-") && (details.get("domain").is_some() || path != "/"))
        || ((prefix_name.starts_with("__http-") || prefix_name.starts_with("__host-http-"))
            && !http_only)
    {
        return Err(error("Cookie prefix requirements were not met."));
    }
    let same_site = match details["sameSite"].as_str().unwrap_or("unspecified") {
        "strict" => soup::SameSitePolicy::Strict,
        "lax" | "unspecified" => soup::SameSitePolicy::Lax,
        "no_restriction" if secure => soup::SameSitePolicy::None,
        _ => return Err(error("SameSite=None cookies require Secure.")),
    };
    let mut cookie = soup::Cookie::new(name, value, &domain, path, -1);
    cookie.set_secure(secure);
    cookie.set_http_only(http_only);
    cookie.set_same_site_policy(same_site);
    if let Some(expires) = details["expirationDate"].as_f64() {
        if !expires.is_finite() || !(-62_135_596_800.0..=253_402_300_799.0).contains(&expires) {
            return Err(error("Cookie expiry is outside the native range."));
        }
        let expires = glib::DateTime::from_unix_utc(expires.floor() as i64)
            .map_err(|_| error("Invalid cookie expiry."))?;
        cookie.set_expires(&expires);
    }
    Ok(cookie)
}

async fn perform(
    inner: &Inner,
    sender: &Sender,
    request: &Request,
    revision: u64,
) -> Result<Value, ApiError> {
    let data = inner
        .context
        .website_data_manager()
        .ok_or_else(|| error("The profile cookie manager is unavailable."))?;
    let matched = inner.tabs.borrow().values().any(|tab| {
        tab.view.upgrade().is_some_and(|view| {
            view.context().as_ref() == Some(&inner.context)
                && view.website_data_manager().as_ref() == Some(&data)
        })
    });
    if !matched {
        return Err(error(
            "No ordinary tab has established this profile's cookie store.",
        ));
    }
    let manager = data
        .cookie_manager()
        .ok_or_else(|| error("The profile cookie manager is unavailable."))?;
    if request.operation == Operation::Stores {
        return inner
            .bridge
            .with_runtime(|host, _| host.cookie_stores(sender))
            .unwrap_or_else(|_| Err(ApiError::context_invalidated()));
    }
    let url = request
        .url
        .as_ref()
        .ok_or_else(|| error("Cookie URL is required."))?;
    if request.operation == Operation::Set {
        let mut cookie = make(request)?;
        let expected = project(&mut cookie)?;
        let expired = cookie.expires().is_some_and(|expiry| {
            expiry.to_unix()
                <= std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
        });
        let mut read_url = url.clone();
        read_url.set_path(expected["path"].as_str().unwrap_or("/"));
        if !allowed(inner, sender, Some(&read_url), revision) {
            return Err(ApiError::context_invalidated());
        }
        manager
            .add_cookie_future(&mut cookie)
            .await
            .map_err(|_| error("Native cookie write failed."))?;
        if !allowed(inner, sender, Some(&read_url), revision) {
            return Err(ApiError::context_invalidated());
        }
        let rows = read(&manager, &read_url).await?;
        let found = rows.into_iter().find(|(_, row)| {
            ["name", "domain", "path"]
                .iter()
                .all(|key| row[key] == expected[key])
        });
        if expired {
            if found.is_some() {
                return Err(error("Cookie expiry was not confirmed."));
            }
        } else if !found.is_some_and(|(_, row)| row == expected) {
            return Err(error("The native cookie write could not be confirmed."));
        }
        if !allowed(inner, sender, Some(url), revision) {
            return Err(ApiError::context_invalidated());
        }
        return Ok(selected(
            read(&manager, url).await?,
            expected["name"].as_str().unwrap_or(""),
        )?
        .map(|(_, row)| row)
        .unwrap_or(Value::Null));
    }
    let rows = read(&manager, url).await?;
    if !allowed(inner, sender, request.url.as_ref(), revision) {
        return Err(ApiError::context_invalidated());
    }
    if request.operation == Operation::GetAll {
        return Ok(rows
            .into_iter()
            .filter(|(_, row)| matches(row, &request.details))
            .map(|(_, row)| row)
            .collect());
    }
    let name = request.details["name"].as_str().unwrap_or("");
    if request.operation == Operation::Get {
        return Ok(selected(rows, name)?
            .map(|(_, row)| row)
            .unwrap_or(Value::Null));
    }
    let matching: Vec<_> = rows
        .into_iter()
        .filter(|(_, row)| row["name"] == name)
        .collect();
    if matching.is_empty() {
        return Ok(json!({"url":url.as_str(), "name":name, "storeId":"0"}));
    }
    for (mut cookie, _) in matching {
        if !allowed(inner, sender, Some(url), revision) {
            return Err(ApiError::context_invalidated());
        }
        manager
            .delete_cookie_future(&mut cookie)
            .await
            .map_err(|_| error("Native cookie deletion failed."))?;
    }
    if !allowed(inner, sender, Some(url), revision) {
        return Err(ApiError::context_invalidated());
    }
    let rows = read(&manager, url).await?;
    if rows.iter().any(|(_, candidate)| candidate["name"] == name) {
        return Err(error("Native cookie deletion was not confirmed."));
    }
    Ok(json!({"url":url.as_str(), "name":name, "storeId":"0"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(details: Value) -> Request {
        Request {
            operation: Operation::Set,
            url: Some(url::Url::parse("https://sub.example.com/account/page").unwrap()),
            details,
        }
    }

    #[test]
    fn cookie_validation_and_projection_preserve_native_evidence() {
        for details in [
            json!({"name":"x","domain":"com"}),
            json!({"name":"x","domain":"other.test"}),
            json!({"name":"x","domain":"..example.com"}),
            json!({"name":"x","domain":"example%2ecom"}),
            json!({"name":"x","path":"/a/../b"}),
            json!({"name":"x","path":"/a?b"}),
            json!({"name":"x;bad"}),
            json!({"name":"","value":"__Http-x=hidden"}),
            json!({"name":"__sEcUrE-x"}),
            json!({"name":"__host-x","secure":true,"domain":"example.com"}),
            json!({"name":"__hOsT-hTtP-x","secure":true,"path":"/"}),
            json!({"name":"x","sameSite":"no_restriction"}),
            json!({"name":"x","path":"/a\n"}),
            json!({"name":"x","expirationDate":1e100}),
        ] {
            assert!(make(&request(details.clone())).is_err(), "{details}");
        }
        let mut cookie = make(&request(json!({"name":"__Host-Http-token","value":"test","secure":true,"httpOnly":true,"path":"/","sameSite":"strict"}))).unwrap();
        let row = project(&mut cookie).unwrap();
        assert_eq!(row["hostOnly"], true);
        assert_eq!(row["httpOnly"], true);
        assert_eq!(row["secure"], true);
        assert_eq!(row["sameSite"], "strict");
        assert_eq!(row["session"], true);
        assert!(row.get("expirationDate").is_none());
        let mut cookie = make(&request(json!({"name":"x","domain":"example.com"}))).unwrap();
        let row = project(&mut cookie).unwrap();
        assert_eq!(row["hostOnly"], false);
        assert_eq!(row["domain"], ".example.com");
        assert!(row.get("sameSite").is_none());
    }
}
