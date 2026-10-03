//! Serving a built single-page app out of a `rust-embed` bundle.
//!
//! The UI is a Vite build (`ui/oss/dist`, `ui/ee/web/dist`). Beside `index.html`
//! it writes two sidecar files that this module reads once at startup:
//!
//! - `routes.json` — `{"routes": ["/", "/agents/$agentId", …]}`, the paths the
//!   app owns. `$name` matches exactly one segment.
//! - `csp.json` — `{"script-src": ["'sha256-…'"]}`, the hashes of the inline
//!   scripts in `index.html` (the pre-paint theme script; OSS adds the
//!   analytics loader).
//!
//! Deciding the shell by `routes.json` rather than by the shape of the URL is
//! the point. The previous handler served `index.html` only for paths with no
//! `.` in them, so any id containing a dot — `/agents/my.agent` — fell through
//! to the 404 page even though the app owns that route.
//!
//! Both editions serve the same way, so the logic lives here once and each
//! binary supplies its own embed type. EE wraps it rather than copying it.

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

/// A 404 must never be cacheable — demo.nasiko.dev sits behind Cloudflare,
/// which edge-cached an uncached 404 for an observed 20 minutes, outliving its
/// own fix.
const NO_STORE: &str = "no-store";

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
}

/// Per-edition additions to the Content-Security-Policy.
///
/// OSS's `index.html` loads analytics from a third-party host; EE's does not.
/// Rather than let one edition's needs widen the other's policy, each binary
/// states its own.
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

        Self { routes, csp }
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
/// in `crate::auth::middleware`, for the same reason: "does it contain a dot"
/// cannot tell `/agents/my.agent` from `/app.js`.
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
///     page — with its navigation — rather than a bare server 404. Unknown
///     `/api/*` paths never reach here; `api_not_found` in `lib.rs` claims them
///     ahead of this fallback.
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

    serve_from::<E>(req, spa)
}

fn serve_from<E: RustEmbed>(req: &Request<Body>, spa: &Spa) -> Response {
    // `Uri::path()` is the raw path and rust-embed keys are literal on-disk
    // names, so decode exactly once before lookup or any asset whose name
    // contains a space 404s. Exact-key matching means a decoded `../` cannot
    // escape the embed set.
    let decoded = percent_decode_str(req.uri().path()).decode_utf8_lossy();
    let path = decoded.trim_start_matches('/');

    if !path.is_empty()
        && let Some(file) = E::get(path)
    {
        return asset(req, path, &file, spa);
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
        Some(file) => shell(&file, spa),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CACHE_CONTROL, NO_STORE)],
            "the UI bundle is missing its index.html — run `just build-ui`",
        )
            .into_response(),
    }
}

fn shell(file: &rust_embed::EmbeddedFile, spa: &Spa) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html".to_owned()),
            (header::CACHE_CONTROL, REVALIDATE.to_owned()),
            (header::CONTENT_SECURITY_POLICY, spa.csp().to_owned()),
            (
                header::ETAG,
                format!("\"{}\"", hex::encode(file.metadata.sha256_hash())),
            ),
        ],
        file.data.clone(),
    )
        .into_response()
}

fn asset(req: &Request<Body>, path: &str, file: &rust_embed::EmbeddedFile, spa: &Spa) -> Response {
    let etag = format!("\"{}\"", hex::encode(file.metadata.sha256_hash()));

    // Only `assets/` is content-hashed by Vite. The shell, the two sidecars and
    // anything copied verbatim from `public/` keep their names across builds,
    // so they must revalidate.
    let cache = if path.starts_with("assets/") {
        IMMUTABLE
    } else {
        REVALIDATE
    };

    if req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(etag.as_str())
    {
        return (
            StatusCode::NOT_MODIFIED,
            [
                (header::CACHE_CONTROL, cache.to_owned()),
                (header::ETAG, etag),
            ],
        )
            .into_response();
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

    (headers, file.data.clone()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spa(patterns: &[&str]) -> Spa {
        Spa {
            routes: patterns.iter().map(|p| Spa::parse_pattern(p)).collect(),
            csp: String::new(),
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
