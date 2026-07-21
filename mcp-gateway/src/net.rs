//! SSRF guard for user-supplied MCP server URLs.
//!
//! Users can register generic MCP servers with arbitrary URLs, and the gateway
//! (plus the `/probe` endpoint) makes server-side requests to them. Without a
//! guard, a tenant could point a "server" at `http://localhost`, an internal
//! service, or the cloud metadata endpoint (`169.254.169.254`) and have the
//! platform fetch it on their behalf — a classic SSRF. This rejects any URL that
//! resolves to a loopback / private / link-local / unspecified address.
//!
//! Enforced at **registration** and **probe** time (not the hot path). The
//! DNS-rebinding gap between validation and the later connection is closed by
//! [`GuardedResolver`], installed on the generic-backend client so reqwest
//! connects only to addresses that pass the same check at resolution time.
//!
//! A `GenericMcpProvider` also holds a **second, unguarded** client — this is
//! not a bypass of the guard above. It exists solely for `MCPServerConfig`s
//! whose `trusted` flag is `true`, meaning the connector's `url` was never
//! typed by a user in the first place: it's an `uploaded_build` MCP-server
//! connector, whose address was resolved by the platform's own
//! `ContainerRuntime::endpoint()` after building and deploying the user's
//! uploaded source. Such an address is necessarily internal/private (it's a
//! container on the platform's own Docker network), which is exactly what this
//! guard exists to reject for *user-supplied* URLs — so a live-traffic
//! `trusted` connector must route around it, not through it. `trusted` is
//! computed in exactly one place (`credentials::build_generic_servers`) from
//! the connector's `source_kind` column and is never accepted as external
//! input anywhere (not in `NewConnectorInput`, the HTTP `CreateConnector`
//! body, or any CLI argument) — see `provider/generic.rs::GenericMcpProvider`.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::error::{McpError, Result};

/// True when the `MCP_ALLOW_PRIVATE_URLS` dev/test bypass is set. Read per-call
/// (not cached) so both the registration-time check and the connect-time
/// resolver observe the same value regardless of construction order.
fn private_urls_allowed() -> bool {
    std::env::var("MCP_ALLOW_PRIVATE_URLS").is_ok_and(|v| v == "true" || v == "1")
}

/// A reqwest DNS resolver that rejects any hostname resolving (even partially) to
/// a private/internal address.
///
/// [`validate_public_url`] guards at **registration/probe** time, but the actual
/// backend fetch happens later against the stored hostname — a hostile DNS could
/// return a public IP at registration and a private one (e.g. `169.254.169.254`)
/// at fetch time (DNS rebinding). Installing this resolver on the client used for
/// generic-backend egress closes that gap: reqwest connects to **exactly** the
/// addresses returned here, so the check and the connection share one resolution
/// — no TOCTOU window. It is scoped to the generic-MCP client only; the platform's
/// shared client must still reach internal hosts (agent containers, OTel, …).
#[derive(Debug, Default)]
struct GuardedResolver;

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let allow_private = private_urls_allowed();
            let filtered: Vec<SocketAddr> =
                addrs.filter(|sa| allow_private || !is_blocked_ip(sa.ip())).collect();
            if filtered.is_empty() {
                return Err(format!(
                    "host '{host}' did not resolve to any allowed (public) address"
                )
                .into());
            }
            let iter: Addrs = Box::new(filtered.into_iter());
            Ok(iter)
        })
    }
}

/// Build a reqwest client whose DNS resolution rejects private/internal targets
/// (SSRF + DNS-rebinding hardened), for outbound calls to user-registered MCP
/// backends. Reuses the platform's rustls stack; per-request timeouts are set by
/// the caller. Honors the `MCP_ALLOW_PRIVATE_URLS` dev/test bypass.
pub fn guarded_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .dns_resolver(Arc::new(GuardedResolver))
        .build()
        .expect("failed to build SSRF-guarded reqwest client")
}

/// Restrict an OAuth post-completion redirect to a relative path or a same-origin
/// absolute URL; anything else (other origins, protocol-relative `//host`) falls
/// back to `/`. Prevents the callbacks becoming an open-redirect phishing gadget.
pub fn safe_redirect(dest: &str, gateway_public_url: Option<&str>) -> String {
    if dest.starts_with('/') && !dest.starts_with("//") {
        return dest.to_string();
    }
    if let Ok(target) = reqwest::Url::parse(dest)
        && let Some(base) = gateway_public_url
        && let Ok(base) = reqwest::Url::parse(base)
        && target.origin() == base.origin()
    {
        return dest.to_string();
    }
    "/".to_string()
}

/// Validate that `raw` is an `http(s)` URL whose host does not resolve to a
/// private/internal address. Returns `BadRequest` otherwise.
pub async fn validate_public_url(raw: &str) -> Result<()> {
    // Opt-in bypass for local dev / tests (loopback backends). MUST stay unset in
    // production — leaving it off is what makes the guard effective.
    if private_urls_allowed() {
        return Ok(());
    }

    let url = reqwest::Url::parse(raw)
        .map_err(|e| McpError::BadRequest(format!("invalid url: {e}")))?;

    match url.scheme() {
        "http" | "https" => {}
        s => return Err(McpError::BadRequest(format!("unsupported url scheme '{s}' (use http/https)"))),
    }

    let host = url
        .host_str()
        .ok_or_else(|| McpError::BadRequest("url has no host".to_string()))?;
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return Err(McpError::BadRequest("url host is not allowed (loopback)".to_string()));
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let mut resolved = false;
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| McpError::BadRequest(format!("cannot resolve url host: {e}")))?;
    for sa in addrs {
        resolved = true;
        if is_blocked_ip(sa.ip()) {
            return Err(McpError::BadRequest(
                "url resolves to a disallowed (private/internal) address".to_string(),
            ));
        }
    }
    if !resolved {
        return Err(McpError::BadRequest("url host did not resolve".to_string()));
    }
    Ok(())
}

/// True if `ip` is loopback / private / link-local / unspecified / otherwise
/// non-public (including the IPv4-mapped forms of those).
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            v.is_loopback()
                || v.is_private()
                || v.is_link_local() // 169.254/16 — includes the cloud metadata IP
                || v.is_unspecified()
                || v.is_broadcast()
                || v.is_documentation()
                || (o[0] == 100 && (o[1] & 0xc0) == 0x40) // 100.64/10 carrier-grade NAT
        }
        IpAddr::V6(v) => {
            if v.is_loopback() || v.is_unspecified() || v.is_multicast() {
                return true;
            }
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_blocked_ip(IpAddr::V4(v4));
            }
            let s = v.segments();
            (s[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (s[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn blocks_private_and_metadata_ips() {
        for ip in [
            "127.0.0.1", "10.0.0.5", "192.168.1.1", "172.16.0.1", "169.254.169.254", "0.0.0.0",
            "100.64.0.1",
        ] {
            assert!(is_blocked_ip(ip.parse::<Ipv4Addr>().unwrap().into()), "{ip} should be blocked");
        }
        assert!(is_blocked_ip(Ipv6Addr::LOCALHOST.into()));
        assert!(is_blocked_ip("fe80::1".parse::<Ipv6Addr>().unwrap().into()));
        assert!(is_blocked_ip("fc00::1".parse::<Ipv6Addr>().unwrap().into()));
        assert!(is_blocked_ip("::ffff:127.0.0.1".parse::<Ipv6Addr>().unwrap().into()));
    }

    #[test]
    fn allows_public_ips() {
        for ip in ["8.8.8.8", "1.1.1.1", "93.184.216.34"] {
            assert!(!is_blocked_ip(ip.parse::<Ipv4Addr>().unwrap().into()), "{ip} should be allowed");
        }
        assert!(!is_blocked_ip("2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap().into()));
    }

    #[test]
    fn safe_redirect_allows_relative_and_same_origin_only() {
        let base = Some("https://app.nasiko.com");
        // Relative paths are always allowed.
        assert_eq!(safe_redirect("/chat", base), "/chat");
        assert_eq!(safe_redirect("/", base), "/");
        // Same-origin absolute is allowed.
        assert_eq!(safe_redirect("https://app.nasiko.com/done", base), "https://app.nasiko.com/done");
        // Off-origin, protocol-relative, and scheme tricks fall back to "/".
        assert_eq!(safe_redirect("https://evil.example.com", base), "/");
        assert_eq!(safe_redirect("//evil.example.com", base), "/");
        assert_eq!(safe_redirect("http://app.nasiko.com/done", base), "/", "scheme mismatch is off-origin");
        assert_eq!(safe_redirect("javascript:alert(1)", base), "/");
        // With no configured base, only relative paths are allowed.
        assert_eq!(safe_redirect("https://app.nasiko.com/x", None), "/");
        assert_eq!(safe_redirect("/x", None), "/x");
    }

    #[test]
    fn rfc1918_172_range_boundaries_are_exact() {
        // 172.16.0.0/12 — just outside on both sides allowed, just inside blocked.
        assert!(!is_blocked_ip("172.15.255.255".parse::<Ipv4Addr>().unwrap().into()));
        assert!(is_blocked_ip("172.16.0.0".parse::<Ipv4Addr>().unwrap().into()));
        assert!(is_blocked_ip("172.31.255.255".parse::<Ipv4Addr>().unwrap().into()));
        assert!(!is_blocked_ip("172.32.0.0".parse::<Ipv4Addr>().unwrap().into()));
    }

    #[test]
    fn link_local_v4_metadata_range_boundaries() {
        // 169.254.0.0/16 — the whole range (incl. cloud metadata IP) blocked end to end.
        assert!(is_blocked_ip("169.254.0.0".parse::<Ipv4Addr>().unwrap().into()));
        assert!(is_blocked_ip("169.254.169.254".parse::<Ipv4Addr>().unwrap().into()));
        assert!(is_blocked_ip("169.254.255.255".parse::<Ipv4Addr>().unwrap().into()));
        assert!(!is_blocked_ip("169.253.255.255".parse::<Ipv4Addr>().unwrap().into()));
        assert!(!is_blocked_ip("169.255.0.0".parse::<Ipv4Addr>().unwrap().into()));
    }

    #[test]
    fn unique_local_v6_range_boundaries() {
        // fc00::/7 spans fc00:: through fdff:...; both ends blocked, fe00:: allowed.
        assert!(is_blocked_ip("fc00::".parse::<Ipv6Addr>().unwrap().into()));
        assert!(is_blocked_ip("fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff".parse::<Ipv6Addr>().unwrap().into()));
        assert!(!is_blocked_ip("fe00::1".parse::<Ipv6Addr>().unwrap().into()));
        assert!(is_blocked_ip("fe80::1".parse::<Ipv6Addr>().unwrap().into()));
        assert!(!is_blocked_ip("fec0::1".parse::<Ipv6Addr>().unwrap().into())); // fe80::/10 ends before fec0::
    }

    #[test]
    fn ipv4_mapped_ipv6_cannot_bypass_the_guard() {
        // Classic SSRF bypass: private v4 encoded as `::ffff:a.b.c.d`. The guard
        // unwraps `to_ipv4_mapped()` and re-checks, so every private class is caught.
        for ip in ["::ffff:127.0.0.1", "::ffff:10.0.0.1", "::ffff:192.168.1.1", "::ffff:172.16.0.1", "::ffff:169.254.169.254"] {
            assert!(is_blocked_ip(ip.parse::<Ipv6Addr>().unwrap().into()), "{ip} (v4-mapped) should be blocked");
        }
        // A v4-mapped *public* address must still be allowed.
        assert!(!is_blocked_ip("::ffff:8.8.8.8".parse::<Ipv6Addr>().unwrap().into()));
    }

    #[test]
    fn multicast_v6_is_blocked() {
        assert!(is_blocked_ip("ff02::1".parse::<Ipv6Addr>().unwrap().into()));
    }
}
