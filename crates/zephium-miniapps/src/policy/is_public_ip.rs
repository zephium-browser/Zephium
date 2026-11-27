use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Returns true only for globally reachable unicast addresses.
///
/// RFC 1918 checks alone are insufficient: shared, reserved, documentation,
/// benchmarking, multicast, and transition addresses are not public targets.
pub fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_broadcast()
        || address.is_multicast()
        || a == 0
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 240)
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }

    let segments = address.segments();
    let globally_routable_unicast = segments[0] & 0xe000 == 0x2000;
    let documentation = segments[0] == 0x2001 && segments[1] == 0x0db8;
    let documentation_v2 = segments[0] == 0x3fff && segments[1] <= 0x0fff;
    let special_purpose = segments[0] == 0x2001 && segments[1] <= 0x01ff;
    let six_to_four = segments[0] == 0x2002;

    globally_routable_unicast
        && !documentation
        && !documentation_v2
        && !special_purpose
        && !six_to_four
        && !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_unique_local()
        && !address.is_unicast_link_local()
        && !address.is_multicast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_globally_routable_unicast_addresses() {
        assert!(is_public_ip("93.184.216.34".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn refuses_non_global_ipv4_and_ipv6_ranges() {
        for address in [
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "192.0.2.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
            "2001::1",
            "2002:7f00:1::",
            "3fff::1",
            "fd00::1",
            "fe80::1",
        ] {
            let address = address.parse().unwrap();
            assert!(
                !is_public_ip(address),
                "{address} is not globally reachable"
            );
        }
    }
}
