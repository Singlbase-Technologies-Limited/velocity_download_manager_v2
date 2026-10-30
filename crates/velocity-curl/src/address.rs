//! Which connected addresses are this computer's or a private network's (#67, ADR-0006). A link
//! from the browser that connects to one waits for the user to confirm it. The check runs on the
//! address libcurl actually connected to (`CURLOPT_PREREQFUNCTION`), after name resolution, so a
//! name that resolves to a private address, as in DNS rebinding, is caught too.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether `address` is this computer's or a local network's: loopback, private (RFC 1918),
/// shared (RFC 6598), link-local, unspecified, or an IPv6 unique local address, and the same
/// for an IPv4 address mapped into IPv6.
pub fn is_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_local_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_local_v4(v4),
            None => is_local_v6(v6),
        },
    }
}

fn is_local_v4(a: Ipv4Addr) -> bool {
    let [first, second, ..] = a.octets();
    a.is_loopback()
        || a.is_private()
        || a.is_link_local()
        || a.is_unspecified()
        // 0.0.0.0/8 reaches this computer on some systems.
        || first == 0
        // 100.64.0.0/10, shared address space (RFC 6598): a carrier's or a VPN's network.
        || (first == 100 && (second & 0b1100_0000) == 64)
}

fn is_local_v6(a: Ipv6Addr) -> bool {
    a.is_loopback() || a.is_unspecified() || a.is_unique_local() || a.is_unicast_link_local()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(s: &str) -> bool {
        is_local(s.parse().expect("an address"))
    }

    #[test]
    fn this_computer_and_private_networks_are_local() {
        for a in [
            "127.0.0.1",
            "127.255.0.9",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.1",
            "169.254.10.20",
            "100.64.0.1",
            "100.127.255.254",
            "0.0.0.0",
            "0.1.2.3",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:192.168.0.10",
        ] {
            assert!(local(a), "{a}");
        }
    }

    #[test]
    fn public_addresses_are_not_local() {
        for a in [
            "93.184.215.14",
            "8.8.8.8",
            "172.32.0.1",
            "172.15.255.255",
            "100.128.0.1",
            "100.63.255.255",
            "192.169.0.1",
            "2606:2800:21f:cb07:6820:80da:af6b:8b2c",
            "::ffff:8.8.8.8",
        ] {
            assert!(!local(a), "{a}");
        }
    }
}
