//! The manual proxy a person may route pages through. The address is checked
//! down to its characters, since the engine hands it to the network stack,
//! and on Windows into a browser argument string.

use std::net::{Ipv4Addr, Ipv6Addr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyKind {
    /// HTTP CONNECT, for http and https pages alike.
    Http,
    Socks5,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyServer {
    pub kind: ProxyKind,
    /// A name or an address, without the brackets of an IPv6 literal.
    pub host: String,
    pub port: u16,
}

impl ProxyServer {
    /// The browser switch that sends all of a view's traffic through it.
    pub fn browser_argument(&self) -> String {
        let scheme = match self.kind {
            ProxyKind::Http => "http",
            ProxyKind::Socks5 => "socks5",
        };
        let brackets = self.host.contains(':');
        format!(
            "--proxy-server={scheme}://{}{}{}:{}",
            if brackets { "[" } else { "" },
            self.host,
            if brackets { "]" } else { "" },
            self.port
        )
    }
}

/// `host:port`: a DNS name, an IPv4 address or a bracketed IPv6 address, and
/// a port from 1 to 65535 written without padding. No scheme, credentials,
/// path or spaces.
pub fn parse_address(text: &str) -> Option<(String, u16)> {
    let (host, port) = text.rsplit_once(':')?;
    let port_number = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    if port_number.to_string() != port {
        return None;
    }
    let host = match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        Some(literal) => {
            literal.parse::<Ipv6Addr>().ok()?;
            literal
        }
        None if host.parse::<Ipv4Addr>().is_ok() || dns_name(host) => host,
        None => return None,
    };
    Some((host.to_ascii_lowercase(), port_number))
}

fn dns_name(host: &str) -> bool {
    // A last label of digits only is a malformed IPv4 address, never a name.
    !host.is_empty()
        && host.len() <= 253
        && !host
            .rsplit('.')
            .next()
            .is_some_and(|last| last.bytes().all(|byte| byte.is_ascii_digit()))
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// The proxy the stored mode and address ask for, or none for the system's.
/// A manual mode with no usable address is the system's too, never a guess.
pub fn from_settings(mode: &str, address: &str) -> Option<ProxyServer> {
    let kind = match mode {
        "http" => ProxyKind::Http,
        "socks5" => ProxyKind::Socks5,
        _ => return None,
    };
    let (host, port) = parse_address(address)?;
    Some(ProxyServer { kind, host, port })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_names_or_ip_literals_with_a_plain_port() {
        for (text, host, port) in [
            ("proxy.example.com:8080", "proxy.example.com", 8080),
            ("Proxy.Example.com:3128", "proxy.example.com", 3128),
            ("localhost:1080", "localhost", 1080),
            ("192.168.0.100:80", "192.168.0.100", 80),
            ("[::1]:1080", "::1", 1080),
            ("[2001:db8::5]:65535", "2001:db8::5", 65535),
            ("a-b.c1:1", "a-b.c1", 1),
        ] {
            assert_eq!(parse_address(text), Some((host.to_owned(), port)), "{text}");
        }
    }

    #[test]
    fn anything_else_is_refused() {
        for text in [
            "",
            "proxy.example.com",
            ":8080",
            "proxy.example.com:",
            "proxy.example.com:0",
            "proxy.example.com:65536",
            "proxy.example.com:08080",
            "proxy.example.com:+80",
            "proxy.example.com: 80",
            " proxy.example.com:80",
            "http://proxy.example.com:80",
            "user:pass@proxy.example.com:80",
            "proxy.example.com/path:80",
            "proxy example.com:80",
            "proxy.example.com:80 --no-sandbox",
            "proxy.example.com:80\n",
            "-proxy.example.com:80",
            "proxy-.example.com:80",
            "proxy..example.com:80",
            "proxy_host.example.com:80",
            "pröxy.example.com:80",
            "::1:1080",
            "[::1:1080",
            "[proxy.example.com]:80",
            "999.1.1.1.1:80",
        ] {
            assert_eq!(parse_address(text), None, "{text:?}");
        }
        assert_eq!(parse_address(&format!("{}:80", "a".repeat(64))), None);
        assert_eq!(parse_address(&format!("{}.com:80", "a.".repeat(130))), None);
    }

    #[test]
    fn the_system_proxy_is_what_a_mode_without_an_address_means() {
        assert_eq!(from_settings("system", "proxy.example.com:80"), None);
        assert_eq!(from_settings("http", ""), None);
        assert_eq!(from_settings("socks5", "not an address"), None);
        assert_eq!(
            from_settings("socks5", "127.0.0.1:9050"),
            Some(ProxyServer {
                kind: ProxyKind::Socks5,
                host: "127.0.0.1".into(),
                port: 9050
            })
        );
    }

    #[test]
    fn the_browser_switch_names_the_scheme_and_brackets_ipv6() {
        let http = from_settings("http", "proxy.example.com:3128").unwrap();
        assert_eq!(
            http.browser_argument(),
            "--proxy-server=http://proxy.example.com:3128"
        );
        let socks = from_settings("socks5", "[::1]:1080").unwrap();
        assert_eq!(
            socks.browser_argument(),
            "--proxy-server=socks5://[::1]:1080"
        );
    }
}
