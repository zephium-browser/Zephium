//! A website's HTTP sign-in challenge (Basic, Digest, NTLM, Negotiate) on a
//! tab a person reads. These rules are shared by both engines: which
//! challenges may ask, how many wrong passwords are heard, and how the typed
//! credentials are held until the engine takes them.
//!
//! The server's realm text never appears here: it is page-controlled and
//! could impersonate the browser.

use std::fmt;
use std::time::Duration;

use url::Url;
use zeroize::Zeroizing;

/// Wrong passwords heard before the challenge is cancelled without asking.
pub const MAX_HTTP_AUTH_RETRIES: u32 = 3;
/// An unanswered prompt cancels its native request after this long.
pub const HTTP_AUTH_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_HOST_BYTES: usize = 253 + 2;
const MAX_USERNAME_BYTES: usize = 1024;
const MAX_PASSWORD_BYTES: usize = 1024;

/// Scheme, host and port of a challenge's protection space, or of a tab's
/// main-frame address.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AuthOrigin {
    scheme: String,
    host: String,
    port: u16,
}

impl AuthOrigin {
    /// From native protection-space parts. Brackets around an IPv6 literal
    /// are dropped, and a missing port takes the scheme's default.
    pub fn new(scheme: &str, host: &str, port: Option<u16>) -> Option<Self> {
        let scheme = scheme.to_ascii_lowercase();
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host)
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if host.is_empty()
            || host.len() > MAX_HOST_BYTES
            || host.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return None;
        }
        let port = match port.filter(|port| *port != 0) {
            Some(port) => port,
            None => default_port(&scheme)?,
        };
        Some(Self { scheme, host, port })
    }

    /// The origin of an http or https address.
    pub fn of_url(url: &Url) -> Option<Self> {
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        Self::new(url.scheme(), url.host_str()?, url.port_or_known_default())
    }

    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// What the prompt names: the host, with its port when it is not the
    /// scheme's default.
    pub fn display_host(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if default_port(&self.scheme) == Some(self.port) {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

/// One challenge as the engine saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthChallenge {
    pub origin: AuthOrigin,
    /// The proxy between the tab and the site asks, not the site.
    pub proxy: bool,
    /// Plain http: the typed password may cross the network readable.
    pub insecure: bool,
    /// The engine's own count of failed attempts for this challenge, where
    /// it keeps one (WebKit does). Without one (WebView2), a repeat from the
    /// same place in the same navigation counts as a failure.
    pub previous_failures: Option<u32>,
}

/// A prompt shown on a tab, without anything the server wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpAuthPrompt {
    /// Per-view, never zero.
    pub request: u32,
    pub host: String,
    pub insecure: bool,
    pub proxy: bool,
    /// An earlier username or password for this challenge was wrong.
    pub retry: bool,
}

/// A username and password typed into a prompt. Wiped from memory when
/// dropped; never printed.
#[derive(Clone)]
pub struct HttpCredentials {
    username: Zeroizing<String>,
    password: Zeroizing<String>,
}

impl HttpCredentials {
    /// None for an empty or oversized username, an oversized password, or
    /// either holding a NUL. An empty username would let WebView2 fall back
    /// to its own dialog.
    pub fn new(username: String, password: String) -> Option<Self> {
        let (username, password) = (Zeroizing::new(username), Zeroizing::new(password));
        (!username.is_empty()
            && username.len() <= MAX_USERNAME_BYTES
            && password.len() <= MAX_PASSWORD_BYTES
            && !username.contains('\0')
            && !password.contains('\0'))
        .then_some(Self { username, password })
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for HttpCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HttpCredentials(redacted)")
    }
}

/// What the tab's page shows at the main frame: the address being loaded,
/// and the committed one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MainFrame {
    pub in_flight: Option<AuthOrigin>,
    pub committed: Option<AuthOrigin>,
}

impl MainFrame {
    /// Only the main frame's own origin asks; a subresource or another
    /// origin's challenge is cancelled, as Chrome does by default. A proxy
    /// asks for whatever the tab is loading.
    pub fn admits(&self, challenge: &AuthChallenge) -> bool {
        if challenge.proxy {
            return self.in_flight.is_some() || self.committed.is_some();
        }
        self.in_flight.as_ref() == Some(&challenge.origin)
            || self.committed.as_ref() == Some(&challenge.origin)
    }
}

/// The outcome of a new challenge on a tab.
pub enum Admission<R> {
    /// Show this prompt. `replaced` is an older prompt this one cancelled.
    Prompt {
        prompt: HttpAuthPrompt,
        replaced: Option<(u32, R)>,
    },
    /// Cancel the new challenge; nothing is shown. `replaced` as above.
    Cancel { replaced: Option<(u32, R)> },
}

struct Pending<R> {
    request: u32,
    origin: AuthOrigin,
    proxy: bool,
    native: R,
}

/// One tab's challenges. `R` is the engine's handle on the native request;
/// dropping it must cancel that request. At most one is held at a time.
pub struct HttpAuthSession<R> {
    next_request: u32,
    pending: Option<Pending<R>>,
    /// Credentials given in the current navigation, and how many times, so
    /// a repeat challenge from the same place reads as a wrong password.
    answered: Option<(AuthOrigin, bool, u32)>,
}

impl<R> Default for HttpAuthSession<R> {
    fn default() -> Self {
        Self {
            next_request: 1,
            pending: None,
            answered: None,
        }
    }
}

impl<R> HttpAuthSession<R> {
    pub fn pending_request(&self) -> Option<u32> {
        self.pending.as_ref().map(|pending| pending.request)
    }

    pub fn pending_native_mut(&mut self) -> Option<&mut R> {
        self.pending.as_mut().map(|pending| &mut pending.native)
    }

    pub fn admit(
        &mut self,
        challenge: &AuthChallenge,
        main_frame: &MainFrame,
        native: R,
    ) -> (Admission<R>, Option<R>) {
        let replaced = self
            .pending
            .take()
            .map(|pending| (pending.request, pending.native));
        if !main_frame.admits(challenge) {
            return (Admission::Cancel { replaced }, Some(native));
        }
        let failures = challenge.previous_failures.unwrap_or_else(|| {
            self.answered
                .as_ref()
                .filter(|(origin, proxy, _)| {
                    *origin == challenge.origin && *proxy == challenge.proxy
                })
                .map_or(0, |(_, _, count)| *count)
        });
        if failures > MAX_HTTP_AUTH_RETRIES {
            return (Admission::Cancel { replaced }, Some(native));
        }
        let request = self.next_request;
        self.next_request = self.next_request.checked_add(1).unwrap_or(1);
        self.pending = Some(Pending {
            request,
            origin: challenge.origin.clone(),
            proxy: challenge.proxy,
            native,
        });
        let prompt = HttpAuthPrompt {
            request,
            host: challenge.origin.display_host(),
            insecure: challenge.insecure,
            proxy: challenge.proxy,
            retry: failures > 0,
        };
        (Admission::Prompt { prompt, replaced }, None)
    }

    /// The person answered `request`. Returns its native handle when it is
    /// still the one shown.
    pub fn answer(&mut self, request: u32, signed_in: bool) -> Option<R> {
        if self.pending_request() != Some(request) {
            return None;
        }
        let pending = self.pending.take()?;
        if signed_in {
            let count = match &self.answered {
                Some((origin, proxy, count))
                    if *origin == pending.origin && *proxy == pending.proxy =>
                {
                    count.saturating_add(1)
                }
                _ => 1,
            };
            self.answered = Some((pending.origin, pending.proxy, count));
        }
        Some(pending.native)
    }

    /// The prompt waited too long.
    pub fn expire(&mut self, request: u32) -> Option<R> {
        if self.pending_request() != Some(request) {
            return None;
        }
        self.pending.take().map(|pending| pending.native)
    }

    /// The tab started, committed or failed a navigation, or is going away:
    /// the shown prompt is cancelled and wrong-password counting restarts.
    pub fn reset(&mut self) -> Option<(u32, R)> {
        self.answered = None;
        self.pending
            .take()
            .map(|pending| (pending.request, pending.native))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct Native(Rc<Cell<u32>>);
    impl Drop for Native {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn origin(url: &str) -> AuthOrigin {
        AuthOrigin::of_url(&Url::parse(url).unwrap()).unwrap()
    }

    fn challenge(url: &str) -> AuthChallenge {
        let origin = origin(url);
        AuthChallenge {
            insecure: origin.scheme() == "http",
            origin,
            proxy: false,
            previous_failures: None,
        }
    }

    fn loading(url: &str) -> MainFrame {
        MainFrame {
            in_flight: Some(origin(url)),
            committed: None,
        }
    }

    fn prompt<R>(admission: Admission<R>) -> HttpAuthPrompt {
        match admission {
            Admission::Prompt { prompt, .. } => prompt,
            Admission::Cancel { .. } => panic!("expected a prompt"),
        }
    }

    #[test]
    fn only_the_main_frame_origin_asks() {
        let frame = MainFrame {
            in_flight: Some(origin("http://router.lan/admin")),
            committed: Some(origin("https://intranet.example:8443/")),
        };
        assert!(frame.admits(&challenge("http://router.lan/")));
        assert!(frame.admits(&challenge("https://intranet.example:8443/x")));
        // Subresource on another origin, or the same host on another port
        // or scheme, is cancelled.
        assert!(!frame.admits(&challenge("https://cdn.example/")));
        assert!(!frame.admits(&challenge("https://router.lan/")));
        assert!(!frame.admits(&challenge("http://router.lan:8080/")));
        assert!(!frame.admits(&challenge("https://intranet.example/")));
        assert!(!MainFrame::default().admits(&challenge("http://router.lan/")));
        let proxy = AuthChallenge {
            proxy: true,
            ..challenge("http://proxy.corp:3128/")
        };
        assert!(frame.admits(&proxy));
        assert!(!MainFrame::default().admits(&proxy));
    }

    #[test]
    fn native_parts_normalise_to_the_url_origin() {
        assert_eq!(
            AuthOrigin::new("HTTP", "Router.LAN.", None),
            Some(origin("http://router.lan/"))
        );
        assert_eq!(
            AuthOrigin::new("https", "[::1]", Some(443)),
            Some(origin("https://[::1]/"))
        );
        assert_eq!(origin("https://[::1]:8443/").display_host(), "[::1]:8443");
        assert_eq!(origin("http://nas.local:80/").display_host(), "nas.local");
        assert_eq!(AuthOrigin::new("ftp", "host", None), None);
        assert_eq!(AuthOrigin::new("https", "", None), None);
        assert_eq!(AuthOrigin::new("https", "a b", None), None);
        assert_eq!(AuthOrigin::of_url(&Url::parse("file:///x").unwrap()), None);
    }

    #[test]
    fn a_subresource_challenge_is_cancelled_without_a_prompt() {
        let dropped = Rc::new(Cell::new(0));
        let mut session = HttpAuthSession::default();
        let (admission, rejected) = session.admit(
            &challenge("https://tracker.example/"),
            &loading("https://news.example/"),
            Native(dropped.clone()),
        );
        assert!(matches!(admission, Admission::Cancel { replaced: None }));
        drop(rejected);
        assert_eq!(dropped.get(), 1);
        assert_eq!(session.pending_request(), None);
    }

    #[test]
    fn a_newer_challenge_replaces_and_cancels_the_older() {
        let dropped = Rc::new(Cell::new(0));
        let mut session = HttpAuthSession::default();
        let frame = loading("http://router.lan/");
        let (first, _) = session.admit(
            &challenge("http://router.lan/"),
            &frame,
            Native(dropped.clone()),
        );
        assert_eq!(prompt(first).request, 1);
        let (second, kept) = session.admit(
            &challenge("http://router.lan/"),
            &frame,
            Native(dropped.clone()),
        );
        assert!(kept.is_none());
        let Admission::Prompt { prompt, replaced } = second else {
            panic!("expected a prompt");
        };
        assert_eq!(prompt.request, 2);
        let (old, native) = replaced.unwrap();
        assert_eq!(old, 1);
        drop(native);
        assert_eq!(dropped.get(), 1);
        assert_eq!(session.pending_request(), Some(2));
        assert!(session.answer(1, true).is_none());
    }

    #[test]
    fn navigation_close_and_timeout_cancel_the_native_request() {
        let dropped = Rc::new(Cell::new(0));
        let mut session = HttpAuthSession::default();
        let frame = loading("http://router.lan/");
        let _ = session.admit(
            &challenge("http://router.lan/"),
            &frame,
            Native(dropped.clone()),
        );
        drop(session.reset());
        assert_eq!(dropped.get(), 1);

        let _ = session.admit(
            &challenge("http://router.lan/"),
            &frame,
            Native(dropped.clone()),
        );
        let request = session.pending_request().unwrap();
        assert!(session.expire(request + 1).is_none());
        drop(session.expire(request));
        assert_eq!(dropped.get(), 2);
        assert_eq!(session.pending_request(), None);

        let _ = session.admit(
            &challenge("http://router.lan/"),
            &frame,
            Native(dropped.clone()),
        );
        drop(session);
        assert_eq!(dropped.get(), 3);
    }

    #[test]
    fn wrong_passwords_are_heard_three_times_then_cancelled() {
        let dropped = Rc::new(Cell::new(0));
        let mut session = HttpAuthSession::default();
        let frame = loading("http://router.lan/");
        let ask = |session: &mut HttpAuthSession<Native>| {
            session
                .admit(
                    &challenge("http://router.lan/"),
                    &frame,
                    Native(dropped.clone()),
                )
                .0
        };
        let first = prompt(ask(&mut session));
        assert!(!first.retry);
        assert!(first.insecure);
        assert_eq!(first.host, "router.lan");
        let mut request = first.request;
        for _ in 0..MAX_HTTP_AUTH_RETRIES {
            assert!(session.answer(request, true).is_some());
            let retry = prompt(ask(&mut session));
            assert!(retry.retry);
            request = retry.request;
        }
        assert!(session.answer(request, true).is_some());
        assert!(matches!(ask(&mut session), Admission::Cancel { .. }));

        // A new navigation starts counting again.
        drop(session.reset());
        assert!(!prompt(ask(&mut session)).retry);
    }

    #[test]
    fn the_engines_own_failure_count_marks_a_retry_and_caps_it() {
        let mut session = HttpAuthSession::<()>::default();
        let frame = loading("https://nas.local/");
        let mut seen = challenge("https://nas.local/");
        seen.previous_failures = Some(1);
        assert!(prompt(session.admit(&seen, &frame, ()).0).retry);
        seen.previous_failures = Some(MAX_HTTP_AUTH_RETRIES + 1);
        assert!(matches!(
            session.admit(&seen, &frame, ()).0,
            Admission::Cancel { .. }
        ));
        // Where the engine counts, a new attempt after the cap asks again
        // even if no navigation was seen in between.
        for _ in 0..=MAX_HTTP_AUTH_RETRIES {
            seen.previous_failures = Some(0);
            let request = prompt(session.admit(&seen, &frame, ()).0).request;
            assert!(session.answer(request, true).is_some());
        }
        seen.previous_failures = Some(0);
        assert!(!prompt(session.admit(&seen, &frame, ()).0).retry);
    }

    #[test]
    fn credentials_are_bounded_and_never_printed() {
        let make =
            |user: &str, password: &str| HttpCredentials::new(user.to_owned(), password.to_owned());
        let credentials = make("admin", "hunter2").unwrap();
        assert_eq!(format!("{credentials:?}"), "HttpCredentials(redacted)");
        assert_eq!(credentials.username(), "admin");
        assert!(make("", "hunter2").is_none());
        assert!(make("a\0b", "x").is_none());
        assert!(make("admin", &"x".repeat(MAX_PASSWORD_BYTES + 1)).is_none());
        assert!(make("admin", "").is_some());
    }
}
