//! Host classification for transport decisions.
//!
//! One definition of "this address is private", shared by everything that has
//! to decide whether plaintext is acceptable on a given hop. Kept together
//! because the answers must not drift: a range that counts as private when a
//! config file is validated has to count as private when the CLI later talks
//! to that same host.

use std::net::{IpAddr, ToSocketAddrs};

/// Whether an address is one plaintext can be tolerated for: loopback, or a
/// range that cannot be routed across the public internet.
///
/// Covers RFC1918, link-local (including the cloud metadata address), CGNAT —
/// where a managed Kubernetes provider's node network commonly lives — and
/// IPv6 unique-local.
pub fn is_private_ip(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback()
                || v4.is_link_local()
                || a == 10
                || (a == 172 && (16..32).contains(&b))
                || (a == 192 && b == 168)
                || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// Whether a host (optionally `host:port`) is a loopback name or a literal
/// private address.
///
/// Deliberately narrow: only `localhost` is accepted as a *name*, so the
/// exemption cannot be widened by pointing a DNS record at a private IP and
/// then moving it later. Callers that can afford to resolve — because they
/// are about to connect anyway, and a failure is immediate and visible — want
/// [`resolves_only_private`] instead.
pub fn is_non_routable_host(host: &str) -> bool {
    let name = strip_port(host);
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    name.parse::<IpAddr>().is_ok_and(is_private_ip)
}

/// Whether a host resolves, and every address it resolves to is private.
///
/// "Every" rather than "any": a name that answers with both a private and a
/// public address would otherwise let plaintext out onto the internet
/// depending on which record a resolver happened to return first.
///
/// Returns `false` when the name does not resolve at all — an unreachable
/// host is not a private one, and failing closed keeps a typo from being
/// treated as an intranet address.
pub fn resolves_only_private(host: &str) -> bool {
    let name = strip_port(host);
    if let Ok(ip) = name.parse::<IpAddr>() {
        return is_private_ip(ip);
    }
    // Port 0 is fine — only the address family matters here.
    match (name, 0u16).to_socket_addrs() {
        Ok(addrs) => {
            let mut any = false;
            for addr in addrs {
                any = true;
                if !is_private_ip(addr.ip()) {
                    return false;
                }
            }
            any
        }
        Err(_) => false,
    }
}

/// Strips an optional `:port` and IPv6 brackets, leaving the bare host.
fn strip_port(host: &str) -> &str {
    // `[::1]:8080` / `[::1]` — the brackets delimit the host unambiguously.
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("");
    }
    // A bare IPv6 literal has more than one colon and cannot carry a port;
    // splitting on the last one turns `::1` into `::`, which parses as the
    // unspecified address and is not loopback.
    if host.matches(':').count() > 1 {
        return host;
    }
    host.rsplit_once(':').map_or(host, |(n, p)| {
        if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
            n
        } else {
            host
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_literals_and_loopback_names_are_non_routable() {
        for host in [
            "localhost",
            "127.0.0.1",
            "::1",
            "10.0.0.5",
            "172.16.3.4",
            "192.168.1.1",
            "100.64.0.1",
            "169.254.169.254",
            "10.0.0.5:30080",
            "[fd00::1]",
        ] {
            assert!(is_non_routable_host(host), "{host} should be non-routable");
        }
    }

    #[test]
    fn public_addresses_and_names_are_routable() {
        for host in [
            "example.com",
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "nasiko.10.0.0.5.nip.io",
        ] {
            assert!(!is_non_routable_host(host), "{host} should be routable");
        }
    }

    #[test]
    fn resolves_only_private_accepts_literals_without_dns() {
        assert!(resolves_only_private("10.1.2.3"));
        assert!(resolves_only_private("10.1.2.3:30080"));
        assert!(!resolves_only_private("93.184.216.34"));
    }

    #[test]
    fn unresolvable_names_are_not_private() {
        // Reserved for exactly this: guaranteed never to resolve.
        assert!(!resolves_only_private("nasiko-does-not-exist.invalid"));
    }
}
