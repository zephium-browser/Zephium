//! A certificate the system did not trust, and the narrow way past one.
//!
//! A person may continue past a certificate error only for a host on their
//! own network: loopback, a private or link-local address, a single-label
//! name, or a reserved local suffix. A public name never qualifies. The
//! exception holds only for the exact scheme, host, port and leaf
//! certificate, lives in memory for one profile until quit, and is never
//! offered to an agent view.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};

use url::Url;

use crate::http_auth::AuthOrigin;
use crate::ids::ProfileId;

/// Exceptions kept per profile; the oldest goes first.
const MAX_EXCEPTIONS_PER_PROFILE: usize = 64;
const LOCAL_SUFFIXES: &[&str] = &["local", "lan", "internal", "home.arpa", "test", "localhost"];

/// What the engine could tell about a certificate it refused, as a category
/// safe to show. Never carries native error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CertificateProblem {
    /// Expired, or not valid yet.
    Expired,
    /// Self-signed or from an issuer the system does not trust.
    Untrusted,
    /// Issued for a different name.
    NameMismatch,
    Revoked,
    Other,
}

/// Whether `host` (as a URL gives it) names something on the person's own
/// network.
pub fn is_local_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    if let Ok(address) = host.parse::<Ipv4Addr>() {
        return local_v4(address);
    }
    if let Ok(address) = host.parse::<Ipv6Addr>() {
        if let Some(mapped) = address.to_ipv4_mapped() {
            return local_v4(mapped);
        }
        let first = address.segments()[0];
        return address.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80;
    }
    // Anything else must be a DNS name; a bare number or stray character is
    // not one.
    if !host.split('.').all(|label| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) || host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        return false;
    }
    !host.contains('.')
        || LOCAL_SUFFIXES.iter().any(|suffix| {
            host == *suffix
                || host
                    .strip_suffix(suffix)
                    .is_some_and(|rest| rest.ends_with('.'))
        })
}

fn local_v4(address: Ipv4Addr) -> bool {
    let [a, b, ..] = address.octets();
    address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        // Carrier-grade NAT, 100.64.0.0/10.
        || (a == 100 && (64..128).contains(&b))
}

/// Whether a failed https address may offer the way past its certificate.
pub fn may_continue(url: &Url) -> bool {
    url.scheme() == "https" && url.host_str().is_some_and(is_local_host)
}

/// One accepted certificate: the origin it was presented for and the SHA-256
/// of its leaf's DER encoding.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CertificateException {
    pub origin: AuthOrigin,
    pub leaf_sha256: [u8; 32],
}

impl CertificateException {
    /// Only an https address on a local host can carry an exception.
    pub fn new(url: &Url, leaf_sha256: [u8; 32]) -> Option<Self> {
        may_continue(url).then_some(())?;
        Some(Self {
            origin: AuthOrigin::of_url(url)?,
            leaf_sha256,
        })
    }
}

/// One profile's exceptions.
#[derive(Clone, Debug, Default)]
pub struct CertificateExceptions {
    accepted: Vec<CertificateException>,
}

impl CertificateExceptions {
    pub fn insert(&mut self, exception: CertificateException) {
        if self.accepted.contains(&exception) {
            return;
        }
        if self.accepted.len() >= MAX_EXCEPTIONS_PER_PROFILE {
            self.accepted.remove(0);
        }
        self.accepted.push(exception);
    }

    /// Whether any exception names this origin, before the leaf is read.
    pub fn covers(&self, scheme: &str, host: &str, port: u16) -> bool {
        AuthOrigin::new(scheme, host, Some(port))
            .is_some_and(|origin| self.accepted.iter().any(|known| known.origin == origin))
    }

    /// Whether the exact origin and leaf were accepted.
    pub fn allows(&self, scheme: &str, host: &str, port: u16, leaf_sha256: &[u8; 32]) -> bool {
        AuthOrigin::new(scheme, host, Some(port)).is_some_and(|origin| {
            self.accepted
                .iter()
                .any(|known| known.origin == origin && known.leaf_sha256 == *leaf_sha256)
        })
    }

    pub fn is_empty(&self) -> bool {
        self.accepted.is_empty()
    }

    pub fn clear(&mut self) {
        self.accepted.clear();
    }
}

/// Every profile's exceptions, kept apart: a private profile's never reach
/// another, and go when its session ends.
#[derive(Clone, Debug, Default)]
pub struct ProfileCertificateExceptions {
    profiles: HashMap<ProfileId, CertificateExceptions>,
}

impl ProfileCertificateExceptions {
    pub fn insert(&mut self, profile: ProfileId, exception: CertificateException) {
        self.profiles.entry(profile).or_default().insert(exception);
    }

    pub fn profile(&self, profile: ProfileId) -> Option<&CertificateExceptions> {
        self.profiles.get(&profile)
    }

    /// Whether the page at `url` in `profile` loads under an exception.
    pub fn covers_url(&self, profile: ProfileId, url: &Url) -> bool {
        let Some(origin) = AuthOrigin::of_url(url) else {
            return false;
        };
        self.profiles.get(&profile).is_some_and(|exceptions| {
            exceptions.covers(origin.scheme(), origin.host(), origin.port())
        })
    }

    pub fn forget(&mut self, profile: ProfileId) -> bool {
        self.profiles
            .remove(&profile)
            .is_some_and(|gone| !gone.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).unwrap()
    }

    #[test]
    fn local_hosts_are_on_the_persons_own_network() {
        for host in [
            "localhost",
            "app.localhost",
            "127.0.0.1",
            "127.8.9.10",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.1",
            "100.64.0.1",
            "100.127.255.254",
            "169.254.10.20",
            "[::1]",
            "::1",
            "[fd12:3456::1]",
            "[fc00::1]",
            "[fe80::1]",
            "[febf::1]",
            "[::ffff:192.168.1.1]",
            "router",
            "nas-01",
            "nas.local",
            "printer.lan",
            "build.corp.internal",
            "media.home.arpa",
            "site.test",
            "NAS.LOCAL.",
        ] {
            assert!(is_local_host(host), "{host} should be local");
        }
    }

    #[test]
    fn public_hosts_and_lookalikes_never_are() {
        for host in [
            "example.com",
            "local.example.com",
            "lan.example.com",
            "example.local.com",
            "192.168.1.1.nip.io",
            "10.0.0.1.sslip.io",
            "home.arpa.example",
            "8.8.8.8",
            "172.32.0.1",
            "172.15.255.255",
            "100.63.255.255",
            "100.128.0.1",
            "169.255.0.1",
            "11.0.0.1",
            "[2001:db8::1]",
            "[2606:4700::1111]",
            "[fec0::1]",
            "[::ffff:8.8.8.8]",
            "",
            "1234",
            "under_score",
        ] {
            assert!(!is_local_host(host), "{host} should not be local");
        }
    }

    #[test]
    fn only_https_on_a_local_host_may_continue() {
        assert!(may_continue(&url("https://192.168.1.1/")));
        assert!(may_continue(&url("https://nas.local:5001/")));
        assert!(may_continue(&url("https://[fd00::1]/")));
        assert!(!may_continue(&url("http://192.168.1.1/")));
        assert!(!may_continue(&url("https://example.com/")));
        assert!(!may_continue(&url("https://local.example.com/")));
        assert!(!may_continue(&url("https://192.168.1.1.nip.io/")));
        assert!(CertificateException::new(&url("https://example.com/"), [1; 32]).is_none());
    }

    #[test]
    fn an_exception_holds_only_for_its_origin_and_certificate() {
        let mut exceptions = CertificateExceptions::default();
        exceptions
            .insert(CertificateException::new(&url("https://nas.local:5001/"), [7; 32]).unwrap());
        assert!(exceptions.covers("https", "nas.local", 5001));
        assert!(exceptions.allows("https", "NAS.local", 5001, &[7; 32]));
        assert!(
            !exceptions.allows("https", "nas.local", 5001, &[8; 32]),
            "another certificate"
        );
        assert!(
            !exceptions.allows("https", "nas.local", 443, &[7; 32]),
            "another port"
        );
        assert!(!exceptions.covers("https", "nas.local", 5002));
        assert!(
            !exceptions.allows("https", "other.local", 5001, &[7; 32]),
            "another host"
        );
        assert!(
            !exceptions.allows("http", "nas.local", 5001, &[7; 32]),
            "another scheme"
        );
    }

    #[test]
    fn exceptions_are_bounded() {
        let mut exceptions = CertificateExceptions::default();
        for index in 0..=MAX_EXCEPTIONS_PER_PROFILE {
            let leaf = [u8::try_from(index % 256).unwrap(); 32];
            exceptions.insert(CertificateException::new(&url("https://nas.local/"), leaf).unwrap());
        }
        assert!(!exceptions.allows("https", "nas.local", 443, &[0; 32]));
        assert!(exceptions.allows("https", "nas.local", 443, &[64; 32]));
    }

    #[test]
    fn a_private_profiles_exceptions_stay_its_own() {
        let regular = ProfileId::generate();
        let private = ProfileId::generate();
        let mut profiles = ProfileCertificateExceptions::default();
        let page = url("https://192.168.1.1/admin");
        profiles.insert(private, CertificateException::new(&page, [3; 32]).unwrap());
        assert!(profiles.covers_url(private, &page));
        assert!(!profiles.covers_url(regular, &page));
        assert!(profiles
            .profile(regular)
            .is_none_or(|exceptions| !exceptions.allows("https", "192.168.1.1", 443, &[3; 32])));
        assert!(profiles.forget(private));
        assert!(!profiles.covers_url(private, &page));
        assert!(!profiles.forget(private));
    }
}
