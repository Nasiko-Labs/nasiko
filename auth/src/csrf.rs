//! Cross-origin write protection for credentials the browser attaches on its
//! own: a session cookie, or HTTP Basic credentials it cached after a
//! `WWW-Authenticate` prompt. Either is sent on a request another page
//! started, so a server that accepts it must also check where the request
//! came from.
//!
//! `SameSite` cookies do not cover this on their own. `SameSite` is decided by
//! *site* (scheme + registrable domain), not *origin*, so two subdomains of one
//! domain are the same site and the cookie is still sent between them. Many
//! write routes take no JSON body, so CORS preflight does not stop them either.
//!
//! The check follows the Fetch Metadata approach (OWASP CSRF cheat sheet; the
//! same algorithm as Go's `net/http.CrossOriginProtection`):
//! 1. Safe methods (GET, HEAD, OPTIONS) always pass.
//! 2. An `Origin` listed in `trusted_origins` passes: a front end on another
//!    origin that this server already allows through CORS.
//! 3. If the browser sent `Sec-Fetch-Site`, only `same-origin` and `none`
//!    (typed into the address bar, a bookmark) pass.
//! 4. Otherwise, if it sent `Origin`, its host must equal `Host`.
//! 5. With neither header the request did not come from a browser page (curl,
//!    a server, a registry client), so it passes: no other page can forge it.
//!
//! Apply it only to requests authenticated by an ambient credential. A bearer
//! token set by script is not ambient, and refusing it would break legitimate
//! cross-origin API clients.

use http::{HeaderMap, Method, header};

/// True for a state-changing request that the browser says another origin
/// started. See the module documentation for the rules.
pub fn is_cross_origin_write(
    method: &Method,
    headers: &HeaderMap,
    trusted_origins: &[String],
) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return false;
    }
    let origin = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok());
    if origin.is_some_and(|o| trusted_origins.iter().any(|t| t.eq_ignore_ascii_case(o))) {
        return false;
    }
    if let Some(site) = headers.get("sec-fetch-site") {
        let site = site.to_str().unwrap_or("");
        return !(site.eq_ignore_ascii_case("same-origin") || site.eq_ignore_ascii_case("none"));
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return false;
    };
    let origin_host = origin
        .to_str()
        .ok()
        .and_then(|o| o.split_once("://"))
        .map(|(_, authority)| authority);
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    match (origin_host, host) {
        (Some(o), Some(h)) => !o.eq_ignore_ascii_case(h),
        // `Origin: null` (sandboxed frames, some redirects), or no `Host`.
        _ => true,
    }
}

/// True when a page's script sent the request (`fetch`, XHR), as opposed to a
/// non-browser client. Only browsers send `Sec-Fetch-Mode`.
///
/// A server that answers a failed Basic login with `WWW-Authenticate: Basic`
/// makes the browser show its own sign-in prompt over the page, and the
/// browser then caches what was typed and attaches it to every later request
/// to that host, including ones another site starts. Leave the challenge off
/// for these requests; registry clients (docker, oras) still get it.
pub fn is_browser_request(headers: &HeaderMap) -> bool {
    headers.contains_key("sec-fetch-mode")
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    fn cross(method: Method, h: &HeaderMap) -> bool {
        is_cross_origin_write(&method, h, &[])
    }

    #[test]
    fn safe_methods_always_pass() {
        let h = headers(&[
            ("sec-fetch-site", "same-site"),
            ("origin", "https://evil.app.nasiko.com"),
        ]);
        for m in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(!cross(m.clone(), &h), "{m}");
        }
    }

    #[test]
    fn fetch_metadata_same_origin_and_none_pass() {
        for site in ["same-origin", "none", "Same-Origin"] {
            let h = headers(&[("sec-fetch-site", site)]);
            assert!(!cross(Method::POST, &h), "{site}");
        }
    }

    #[test]
    fn a_sibling_subdomain_is_refused_even_though_it_is_same_site() {
        for site in ["same-site", "cross-site"] {
            let h = headers(&[("sec-fetch-site", site), ("host", "app.nasiko.com")]);
            assert!(cross(Method::POST, &h), "{site}");
            assert!(cross(Method::DELETE, &h), "{site}");
        }
    }

    #[test]
    fn fetch_metadata_wins_over_a_matching_origin() {
        let h = headers(&[
            ("sec-fetch-site", "same-site"),
            ("origin", "https://app.nasiko.com"),
            ("host", "app.nasiko.com"),
        ]);
        assert!(cross(Method::PUT, &h));
    }

    #[test]
    fn without_fetch_metadata_origin_must_match_host() {
        let same = headers(&[
            ("origin", "https://app.nasiko.com"),
            ("host", "app.nasiko.com"),
        ]);
        assert!(!cross(Method::POST, &same));
        let with_port = headers(&[
            ("origin", "http://localhost:8080"),
            ("host", "localhost:8080"),
        ]);
        assert!(!cross(Method::POST, &with_port));
        let sibling = headers(&[
            ("origin", "https://acme.app.nasiko.com"),
            ("host", "app.nasiko.com"),
        ]);
        assert!(cross(Method::POST, &sibling));
        let null = headers(&[("origin", "null"), ("host", "app.nasiko.com")]);
        assert!(cross(Method::POST, &null));
    }

    #[test]
    fn a_request_with_neither_header_is_not_from_a_browser_page() {
        assert!(!cross(
            Method::POST,
            &headers(&[("host", "app.nasiko.com")])
        ));
    }

    #[test]
    fn a_trusted_origin_passes_and_others_do_not() {
        let trusted = ["https://app.nasiko.com".to_string()];
        let from_trusted = headers(&[
            ("sec-fetch-site", "same-site"),
            ("origin", "https://app.nasiko.com"),
            ("host", "registry.nasiko.com"),
        ]);
        assert!(!is_cross_origin_write(
            &Method::POST,
            &from_trusted,
            &trusted
        ));
        let from_sibling = headers(&[
            ("sec-fetch-site", "same-site"),
            ("origin", "https://acme.app.nasiko.com"),
            ("host", "registry.nasiko.com"),
        ]);
        assert!(is_cross_origin_write(
            &Method::POST,
            &from_sibling,
            &trusted
        ));
    }

    #[test]
    fn only_browsers_are_browser_requests() {
        assert!(is_browser_request(&headers(&[("sec-fetch-mode", "cors")])));
        assert!(!is_browser_request(&headers(&[(
            "user-agent",
            "docker/27.0"
        )])));
    }
}
