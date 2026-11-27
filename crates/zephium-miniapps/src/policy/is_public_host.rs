/// Installation talks to the public web, never to the person's own network.
/// The check runs on the typed address and again on the address a redirect
/// actually landed on, so neither the first request nor a later hop can probe a
/// loopback, link-local, or private destination.
pub fn is_public_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let bare = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(&host);
    if bare == "localhost"
        || bare.ends_with(".localhost")
        || bare.ends_with(".local")
        || bare.ends_with(".internal")
    {
        return false;
    }
    let Ok(address) = bare.parse::<std::net::IpAddr>() else {
        return true;
    };
    super::is_public_ip::is_public_ip(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_public_names_and_addresses() {
        assert!(is_public_host("example.com"));
        assert!(is_public_host("sub.example.com."));
        assert!(is_public_host("93.184.216.34"));
        assert!(is_public_host("[2606:4700:4700::1111]"));
    }

    #[test]
    fn refuses_names_and_addresses_that_point_inward() {
        for host in [
            "localhost",
            "app.localhost",
            "printer.local",
            "build.internal",
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.10",
            "169.254.169.254",
            "100.64.0.1",
            "203.0.113.10",
            "0.0.0.0",
            "[::1]",
            "[fd00::1]",
            "[fe80::1]",
        ] {
            assert!(!is_public_host(host), "{host} must not be installed from");
        }
    }
}
