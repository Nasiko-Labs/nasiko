//! Serving a built single-page app out of a `rust-embed` bundle.
//!
//! The UI is a Vite build (`ui/oss/app/dist`, `ui/ee/app/dist`, …). Beside `index.html`
//! it writes two sidecar files that this module reads once at startup:
//!
//! - `routes.json` — `{"routes": ["/", "/agents/$agentId", …]}`, the paths the
//!   app owns. `$name` matches exactly one segment.
//! - `csp.json` — `{"script-src": ["'sha256-…'"]}`, the hashes of the inline
//!   scripts in `index.html` (the pre-paint theme script and the
//!   analytics loader).
//!
//! Deciding the shell by `routes.json` rather than by the shape of the URL is
//! the point. The previous handler served `index.html` only for paths with no
//! `.` in them, so any id containing a dot — `/agents/my.agent` — fell through
//! to the 404 page even though the app owns that route.
//!
//! The build also writes `<file>.br` and `<file>.gz` beside each text asset
//! (`ui/scripts/precompress.ts`). Nothing is compressed at run time: a request
//! gets the variant its `Accept-Encoding` allows, else the original.
//!
//! Every binary that embeds a UI bundle serves it the same way, so the logic
//! lives here once and each binary supplies its own embed type and CSP extras.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::percent_decode_str;
use rust_embed::RustEmbed;
use serde::Deserialize;
use std::sync::OnceLock;

/// Asset filenames carry a content hash, so a given URL's bytes never change
/// and the browser need never ask again.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// The shell and its sidecars are the one unhashed pair: they must be
/// revalidated on every navigation or a deploy stays invisible.
const REVALIDATE: &str = "no-cache";

/// Precompressed variants the build writes beside a text asset
/// (`ui/scripts/precompress.ts`), in order of preference: the
/// `Content-Encoding` token and the file suffix. Only this handler serves them,
/// so API and SSE responses are never touched. A bundle without variants
/// (build.rs's placeholder) is served as before.
const ENCODINGS: &[(&str, &str)] = &[("br", ".br"), ("gzip", ".gz")];

/// Marks a response from a bundle that carries precompressed variants, so a
/// runtime compression layer in front of the handler leaves it alone: one owner
/// of `Content-Encoding`, no second pass over formats that are compressed
/// already, and each representation keeps its own validator. EE's static layer
/// checks for it (`ee/server/src/main.rs`); a bundle with no variants is not
/// marked and keeps that layer's compression.
#[derive(Clone, Copy, Debug)]
pub struct Precompressed;

/// A 404 must never be cacheable — demo.nasiko.dev sits behind Cloudflare,
/// which edge-cached an uncached 404 for an observed 20 minutes, outliving its
/// own fix.
const NO_STORE: &str = "no-store";

/// Browser hardening on everything the bundle serves: the shell, its assets
/// and the 404s. The opener policy allows popups because the MCP OAuth sign-in
/// watches its popup's `closed` (`ui/shared/features/src/mcp/oauth.ts`), and
/// `same-origin` severs that handle as soon as the provider's page loads.
const HARDENING: [(&str, &str); 5] = [
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "strict-origin-when-cross-origin"),
    ("x-frame-options", "DENY"),
    (
        "permissions-policy",
        "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
    ),
    ("cross-origin-opener-policy", "same-origin-allow-popups"),
];

#[derive(Deserialize, Default)]
struct RoutesFile {
    #[serde(default)]
    routes: Vec<String>,
}

#[derive(Deserialize, Default)]
struct CspFile {
    #[serde(rename = "script-src", default)]
    script_src: Vec<String>,
}

/// One segment of a route pattern.
#[derive(Debug, PartialEq)]
enum Segment {
    /// A literal path segment, matched exactly.
    Literal(String),
    /// A `$name` placeholder, matching exactly one non-empty segment.
    Param,
}

/// Everything the handler needs to serve one edition's bundle, resolved once at
/// startup so no request pays for parsing.
pub struct Spa {
    routes: Vec<Vec<Segment>>,
    csp: String,
    /// The bundle carries `.br`/`.gz` variants.
    precompressed: bool,
}

/// Per-edition additions to the Content-Security-Policy.
///
/// Each binary states its own extras, so one edition's third-party hosts never
/// widen the other's policy by accident.
#[derive(Default)]
pub struct CspExtras {
    pub script_src: &'static [&'static str],
    pub connect_src: &'static [&'static str],
    pub img_src: &'static [&'static str],
}

impl Spa {
    /// Read `routes.json` and `csp.json` out of the embedded bundle.
    ///
    /// A bundle that has neither (the `build.rs` placeholder, written when
    /// nobody has run `npm run build`) yields an empty route table. The handler
    /// treats that as "every extensionless path is the shell", which is what
    /// the placeholder wants — it is a single page telling you to build.
    pub fn load<E: RustEmbed>(extras: &CspExtras) -> Self {
        let routes = Self::read::<E, RoutesFile>("routes.json")
            .routes
            .iter()
            .map(|r| Self::parse_pattern(r))
            .collect();

        let csp = Self::build_csp(&Self::read::<E, CspFile>("csp.json").script_src, extras);
        let precompressed = ENCODINGS
            .iter()
            .any(|(_, suffix)| E::get(&format!("index.html{suffix}")).is_some());

        Self {
            routes,
            csp,
            precompressed,
        }
    }

    fn read<E: RustEmbed, T: Default + serde::de::DeserializeOwned>(name: &str) -> T {
        let Some(file) = E::get(name) else {
            tracing::warn!(
                "{name} is missing from the embedded UI bundle — run `just build-ui`. \
                 Serving with defaults until then."
            );
            return T::default();
        };
        serde_json::from_slice(&file.data).unwrap_or_else(|e| {
            tracing::error!("{name} in the embedded UI bundle is not valid JSON: {e}");
            T::default()
        })
    }

    /// `/agents/$agentId` → `[Literal("agents"), Param]`.
    fn parse_pattern(pattern: &str) -> Vec<Segment> {
        pattern
            .trim_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.starts_with('$') {
                    Segment::Param
                } else {
                    Segment::Literal(s.to_owned())
                }
            })
            .collect()
    }

    fn build_csp(hashes: &[String], extras: &CspExtras) -> String {
        let join = |base: &str, extra: &[&str]| {
            if extra.is_empty() {
                base.to_owned()
            } else {
                format!("{base} {}", extra.join(" "))
            }
        };

        let script = format!(
            "'self' {}",
            hashes
                .iter()
                .map(String::as_str)
                .chain(extras.script_src.iter().copied())
                .collect::<Vec<_>>()
                .join(" ")
        );

        // `style-src` keeps `'unsafe-inline'` deliberately: the app sets the
        // `style` attribute in 64 places (bar widths in the trace waterfall,
        // theme swatches, chart geometry) where the value is computed per
        // render and cannot be a class. `style-src-attr` would express that
        // more tightly, but it is CSP Level 3 and a browser without it falls
        // back to `style-src`, which would block those attributes and break
        // the pages silently. Scripts — the part that actually matters — stay
        // hash-pinned with no `'unsafe-inline'`.
        [
            "default-src 'self'".to_owned(),
            format!("script-src {script}"),
            "style-src 'self' 'unsafe-inline'".to_owned(),
            join("img-src 'self' data:", extras.img_src),
            "font-src 'self'".to_owned(),
            join("connect-src 'self'", extras.connect_src),
            "frame-ancestors 'none'".to_owned(),
            "base-uri 'self'".to_owned(),
            "form-action 'self'".to_owned(),
        ]
        .join("; ")
    }

    /// Does the app own this path?
    ///
    /// An empty route table means the bundle shipped without a manifest; fall
    /// back to "anything without a file extension is a page" so a placeholder
    /// build still renders.
    fn owns(&self, path: &str) -> bool {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if self.routes.is_empty() {
            return !path.contains('.');
        }

        self.routes.iter().any(|pattern| {
            pattern.len() == segments.len()
                && pattern.iter().zip(&segments).all(|(p, s)| match p {
                    Segment::Literal(lit) => lit == s,
                    Segment::Param => !s.is_empty(),
                })
        })
    }

    pub fn csp(&self) -> &str {
        &self.csp
    }
}

/// Extensions a browser only ever requests as a subresource. Mirrors the list
/// in `nasiko-server`'s `auth::middleware`, for the same reason: "does it
/// contain a dot" cannot tell `/agents/my.agent` from `/app.js`.
const ASSET_EXTENSIONS: &[&str] = &[
    "css",
    "js",
    "mjs",
    "map",
    "json",
    "svg",
    "png",
    "jpg",
    "jpeg",
    "gif",
    "webp",
    "avif",
    "ico",
    "woff",
    "woff2",
    "ttf",
    "otf",
    "txt",
    "xml",
    "webmanifest",
    "wasm",
];

/// Did the browser ask for a file rather than a page?
fn looks_like_an_asset(path: &str) -> bool {
    if path.starts_with("assets/") {
        return true;
    }
    match path.rsplit('/').next().unwrap_or("").rsplit_once('.') {
        Some((_, ext)) => ASSET_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()),
        None => false,
    }
}

/// Serve one request from `E`'s bundle.
///
/// Order:
///
///  1. A real file wins.
///  2. A path `routes.json` claims gets the shell.
///  3. A path that *looks like an asset* is a genuine 404. This is the case
///     that must not fall through: a stale `/assets/x.js` answered with a 200
///     HTML body is how a cached page ends up executing markup as JavaScript.
///  4. Anything else gets the shell too, so the app renders its own not-found
///     page — with its navigation — rather than a bare server 404. A binary
///     must claim its own API prefixes ahead of this fallback (`nasiko-server`'s
///     `api_not_found`), or an unknown API path comes back as the shell.
pub fn serve<E: RustEmbed>(
    req: &Request<Body>,
    cache: &'static OnceLock<Spa>,
    extras: &CspExtras,
) -> Response {
    // Release parses the manifest once: the bytes are baked into the binary and
    // cannot change while it runs.
    //
    // Debug re-reads it every request, because rust-embed serves from disk
    // there (`debug-embed` is off). That is what lets `just build-ui` refresh
    // the UI with no cargo rebuild — but a cached manifest would not follow, so
    // a route added by that rebuild would 404 until someone restarted the
    // server, which is a confusing way to spend ten minutes.
    #[cfg(debug_assertions)]
    let spa = {
        let _ = cache;
        Spa::load::<E>(extras)
    };
    #[cfg(debug_assertions)]
    let spa = &spa;

    #[cfg(not(debug_assertions))]
    let spa = cache.get_or_init(|| Spa::load::<E>(extras));

    let mut response = serve_from::<E>(req, spa);
    if spa.precompressed {
        response.extensions_mut().insert(Precompressed);
    }
    let headers = response.headers_mut();
    for (name, value) in HARDENING {
        headers.insert(name, HeaderValue::from_static(value));
    }
    response
}

fn serve_from<E: RustEmbed>(req: &Request<Body>, spa: &Spa) -> Response {
    // `Uri::path()` is the raw path and rust-embed keys are literal on-disk
    // names, so decode exactly once before lookup or any asset whose name
    // contains a space 404s. Exact-key matching means a decoded `../` cannot
    // escape the embed set.
    let decoded = percent_decode_str(req.uri().path()).decode_utf8_lossy();
    let path = decoded.trim_start_matches('/');

    // A variant is only ever served as its original's representation, never by
    // its own name (which would carry the wrong type and no Content-Encoding).
    if let Some(original) = ENCODINGS
        .iter()
        .find_map(|(_, suffix)| path.strip_suffix(suffix))
        && E::get(original).is_some()
    {
        return (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, NO_STORE)]).into_response();
    }

    if !path.is_empty()
        && let Some(file) = E::get(path)
    {
        return asset::<E>(req, path, &file, spa);
    }

    if looks_like_an_asset(path) && !spa.owns(&decoded) {
        return (
            StatusCode::NOT_FOUND,
            [
                (header::CACHE_CONTROL, NO_STORE),
                (header::CONTENT_SECURITY_POLICY, spa.csp()),
            ],
        )
            .into_response();
    }

    match E::get("index.html") {
        Some(file) => shell::<E>(req, &file, spa),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CACHE_CONTROL, NO_STORE)],
            "the UI bundle is missing its index.html — run `just build-ui`",
        )
            .into_response(),
    }
}

fn shell<E: RustEmbed>(
    req: &Request<Body>,
    file: &rust_embed::EmbeddedFile,
    spa: &Spa,
) -> Response {
    let (encoding, body) = negotiate::<E>(req.headers(), "index.html", file);
    let etag = etag(file, encoding);
    // `no-cache` means "revalidate", so answer the revalidation: the shell is fetched on every navigation.
    if if_none_match(req.headers(), &etag) {
        return not_modified(REVALIDATE, &etag);
    }
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(REVALIDATE));
    if let Ok(v) = HeaderValue::from_str(spa.csp()) {
        headers.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    if let Ok(v) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, v);
    }
    representation_headers(&mut headers, encoding);
    (headers, body).into_response()
}

fn asset<E: RustEmbed>(
    req: &Request<Body>,
    path: &str,
    file: &rust_embed::EmbeddedFile,
    spa: &Spa,
) -> Response {
    let (encoding, body) = negotiate::<E>(req.headers(), path, file);
    let etag = etag(file, encoding);

    // Only `assets/` is content-hashed by Vite. The shell, the two sidecars and
    // anything copied verbatim from `public/` keep their names across builds,
    // so they must revalidate.
    let cache = if path.starts_with("assets/") {
        IMMUTABLE
    } else {
        REVALIDATE
    };

    if if_none_match(req.headers(), &etag) {
        return not_modified(cache, &etag);
    }

    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, v);
    }
    // The policy belongs on the document, not on every byte it pulls in.
    if mime.as_ref().starts_with("text/html")
        && let Ok(v) = HeaderValue::from_str(spa.csp())
    {
        headers.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    representation_headers(&mut headers, encoding);

    (headers, body).into_response()
}

/// The representation to send: a precompressed variant the client accepts,
/// else the original. Returns the `Content-Encoding` token (if any) and bytes.
fn negotiate<E: RustEmbed>(
    request: &HeaderMap,
    path: &str,
    original: &rust_embed::EmbeddedFile,
) -> (Option<&'static str>, std::borrow::Cow<'static, [u8]>) {
    for (coding, suffix) in ENCODINGS {
        if accepts(request, coding)
            && let Some(variant) = E::get(&format!("{path}{suffix}"))
        {
            return (Some(coding), variant.data);
        }
    }
    (None, original.data.clone())
}

/// Does `Accept-Encoding` allow `coding`? An explicit `q=0` refuses it; a `*`
/// entry stands in for codings the header does not name.
fn accepts(request: &HeaderMap, coding: &str) -> bool {
    let Some(value) = request
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let mut wildcard = None;
    for item in value.split(',') {
        let mut parts = item.split(';');
        let name = parts.next().unwrap_or("").trim();
        let q = parts
            .find_map(|p| p.trim().strip_prefix("q="))
            .map_or(1.0, |q| q.trim().parse::<f32>().unwrap_or(0.0));
        if name.eq_ignore_ascii_case(coding) {
            return q > 0.0;
        }
        if name == "*" {
            wildcard = Some(q > 0.0);
        }
    }
    wildcard.unwrap_or(false)
}

/// Each representation has its own validator: a cached Brotli body must never
/// be revalidated as the identity one.
fn etag(file: &rust_embed::EmbeddedFile, encoding: Option<&str>) -> String {
    let hash = hex::encode(file.metadata.sha256_hash());
    match encoding {
        Some(coding) => format!("\"{hash}-{coding}\""),
        None => format!("\"{hash}\""),
    }
}

/// The response varies by `Accept-Encoding` whenever a variant could have been
/// chosen, so a shared cache keeps the representations apart.
fn representation_headers(headers: &mut HeaderMap, encoding: Option<&'static str>) {
    headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    if let Some(coding) = encoding {
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(coding));
    }
}

fn if_none_match(request: &HeaderMap, etag: &str) -> bool {
    request
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|candidate| candidate.trim() == etag))
}

fn not_modified(cache: &'static str, etag: &str) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(v) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, v);
    }
    headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    (StatusCode::NOT_MODIFIED, headers).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spa(patterns: &[&str]) -> Spa {
        Spa {
            routes: patterns.iter().map(|p| Spa::parse_pattern(p)).collect(),
            csp: String::new(),
            precompressed: false,
        }
    }

    /// A built bundle in miniature: a hashed asset with both variants, a shell with only a Brotli one.
    #[derive(RustEmbed)]
    #[folder = "tests/fixtures/spa/"]
    struct Fixture;

    fn get(path: &str, accept_encoding: Option<&str>, if_none_match: Option<&str>) -> Response {
        static CACHE: OnceLock<Spa> = OnceLock::new();
        let mut req = Request::builder().uri(path);
        if let Some(v) = accept_encoding {
            req = req.header(header::ACCEPT_ENCODING, v);
        }
        if let Some(v) = if_none_match {
            req = req.header(header::IF_NONE_MATCH, v);
        }
        serve::<Fixture>(
            &req.body(Body::empty()).unwrap(),
            &CACHE,
            &CspExtras::default(),
        )
    }

    fn encoding(response: &Response) -> Option<&str> {
        response
            .headers()
            .get(header::CONTENT_ENCODING)
            .map(|v| v.to_str().unwrap())
    }

    const ASSET: &str = "/assets/app-abc123.js";

    #[test]
    fn a_hashed_asset_is_served_in_the_best_encoding_the_client_accepts() {
        let cases = [
            (Some("gzip, deflate, br, zstd"), Some("br")),
            (Some("gzip, deflate"), Some("gzip")),
            (Some("br;q=0, gzip"), Some("gzip")),
            (Some("identity"), None),
            (Some("*"), Some("br")),
            (Some("*;q=0, identity"), None),
            (None, None),
        ];
        for (accept, expected) in cases {
            let response = get(ASSET, accept, None);
            assert_eq!(response.status(), StatusCode::OK, "{accept:?}");
            assert_eq!(encoding(&response), expected, "{accept:?}");
            let headers = response.headers();
            // The type is the original's, whatever the encoding.
            assert!(
                headers[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .contains("javascript"),
                "{accept:?}"
            );
            assert_eq!(headers[header::CACHE_CONTROL], IMMUTABLE);
            assert_eq!(headers[header::VARY], "accept-encoding");
        }
    }

    #[test]
    fn each_encoding_has_its_own_validator() {
        let br = get(ASSET, Some("br"), None);
        let identity = get(ASSET, None, None);
        let br_tag = br.headers()[header::ETAG].to_str().unwrap().to_owned();
        let identity_tag = identity.headers()[header::ETAG]
            .to_str()
            .unwrap()
            .to_owned();
        assert_ne!(br_tag, identity_tag);
        assert!(br_tag.ends_with("-br\""));

        let revalidated = get(ASSET, Some("br"), Some(&br_tag));
        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(revalidated.headers()[header::VARY], "accept-encoding");
        // A Brotli validator never revalidates the identity representation.
        assert_eq!(get(ASSET, None, Some(&br_tag)).status(), StatusCode::OK);
        assert_eq!(
            get(ASSET, Some("gzip"), Some(&format!("\"x\", {identity_tag}"))).status(),
            StatusCode::OK
        );
        assert_eq!(
            get(ASSET, None, Some(&format!("\"x\", {identity_tag}"))).status(),
            StatusCode::NOT_MODIFIED
        );
    }

    #[test]
    fn the_shell_is_negotiated_and_still_revalidates() {
        let response = get("/agents/a1", Some("br, gzip"), None);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(encoding(&response), Some("br"));
        assert_eq!(response.headers()[header::CACHE_CONTROL], REVALIDATE);
        assert!(
            response
                .headers()
                .contains_key(header::CONTENT_SECURITY_POLICY)
        );
        // index.html has no gzip variant in the fixture: identity, not a 404.
        assert_eq!(encoding(&get("/agents/a1", Some("gzip"), None)), None);
        // The same representation's validator gives a 304, another one a full body.
        let tag = response.headers()[header::ETAG]
            .to_str()
            .unwrap()
            .to_owned();
        let revalidated = get("/agents/a1", Some("br"), Some(&tag));
        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(revalidated.headers()[header::CACHE_CONTROL], REVALIDATE);
        assert_eq!(get("/agents/a1", None, Some(&tag)).status(), StatusCode::OK);
    }

    #[test]
    fn responses_from_a_precompressed_bundle_are_marked_for_outer_layers() {
        for (path, accept) in [
            (ASSET, Some("br")),
            (ASSET, None),
            ("/agents/a1", Some("gzip")),
            ("/assets/missing.js", None),
        ] {
            let response = get(path, accept, None);
            assert!(
                response.extensions().get::<Precompressed>().is_some(),
                "{path} {accept:?}"
            );
        }
    }

    #[test]
    fn a_variant_is_never_served_under_its_own_name() {
        for path in [
            "/assets/app-abc123.js.br",
            "/assets/app-abc123.js.gz",
            "/index.html.br",
        ] {
            let response = get(path, Some("br, gzip"), None);
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], NO_STORE);
        }
    }

    #[test]
    fn literal_routes_match_exactly() {
        let s = spa(&["/", "/agents", "/settings/secrets"]);
        assert!(s.owns("/"));
        assert!(s.owns("/agents"));
        assert!(s.owns("/settings/secrets"));
        assert!(!s.owns("/agents/extra"));
        assert!(!s.owns("/settings"));
    }

    /// The reason this module exists: an id with a dot is a route, not a
    /// missing asset. The old handler keyed on `!path.contains('.')`.
    #[test]
    fn a_param_accepts_a_dot() {
        let s = spa(&["/agents/$agentId"]);
        assert!(s.owns("/agents/my.agent"));
        assert!(s.owns("/agents/7f3a-11ee"));
    }

    /// `$name` is one segment, so it must not swallow a deeper path.
    #[test]
    fn a_param_spans_exactly_one_segment() {
        let s = spa(&["/agents/$agentId"]);
        assert!(!s.owns("/agents/a/b"));
        assert!(!s.owns("/agents"));
    }

    #[test]
    fn trailing_slashes_do_not_change_the_match() {
        let s = spa(&["/agents/$agentId"]);
        assert!(s.owns("/agents/abc/"));
    }

    /// A bundle with no manifest is the build.rs placeholder; it still has to
    /// render, so fall back to the old extensionless rule.
    #[test]
    fn an_empty_manifest_falls_back_to_the_extension_rule() {
        let s = spa(&[]);
        assert!(s.owns("/anything"));
        assert!(!s.owns("/assets/app.js"));
    }

    #[test]
    fn an_asset_path_is_recognised_by_extension_not_by_a_dot() {
        assert!(looks_like_an_asset("assets/index-a1b2.js"));
        assert!(looks_like_an_asset("mark-nasiko.svg"));
        assert!(looks_like_an_asset("routes.json"));
        // The case the dot rule got wrong.
        assert!(!looks_like_an_asset("agents/my.agent"));
        assert!(!looks_like_an_asset("sessions/abc"));
        assert!(!looks_like_an_asset(""));
    }

    /// This crate's own sources stand in for a bundle: a real file, an asset
    /// that is gone and a bundle with no shell each take a different path.
    #[derive(RustEmbed)]
    #[folder = "src"]
    struct Sources;

    #[test]
    fn every_response_is_hardened() {
        static SPA: OnceLock<Spa> = OnceLock::new();
        for path in ["/lib.rs", "/assets/gone.js", "/agents"] {
            let req = Request::get(path).body(Body::empty()).expect("request");
            let res = serve::<Sources>(&req, &SPA, &CspExtras::default());
            for (name, value) in HARDENING {
                assert_eq!(res.headers()[name], value, "{name} on {path}");
            }
        }
    }

    #[test]
    fn hashes_and_edition_extras_both_reach_script_src() {
        let csp = Spa::build_csp(
            &["'sha256-abc='".to_owned()],
            &CspExtras {
                script_src: &["https://static.reo.dev"],
                connect_src: &["https://api.reo.dev"],
                ..Default::default()
            },
        );
        assert!(csp.contains("script-src 'self' 'sha256-abc=' https://static.reo.dev"));
        assert!(csp.contains("connect-src 'self' https://api.reo.dev"));
        assert!(csp.contains("frame-ancestors 'none'"));
        // Scripts stay hash-pinned even though styles are relaxed.
        assert!(!csp.contains("script-src 'self' 'unsafe-inline'"));
    }
}
