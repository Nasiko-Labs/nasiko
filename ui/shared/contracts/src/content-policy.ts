/**
 * Content policy for anything rendered from data: links, media and text that
 * came from a backend row, a generated surface or a model. Pure functions so
 * every renderer (composites, extensions, Weave runtime) applies one rule set
 * (contract §17.5 generated-UI protections). Zod-free on purpose: this module
 * ships to the browser; the schemas live in `content-schemas.ts`.
 *
 * Design (Phase 3 correction): every candidate is *normalised by the URL
 * parser* and judged on the resulting origin, protocol and host — never on
 * string prefixes. The earlier prefix rule accepted `/\evil.test/x`, which a
 * browser normalises to `//evil.test/x` (scheme-relative, external). Now a
 * relative href must resolve to the same origin as a fixed internal base;
 * an absolute href must carry an allowed protocol and, for http(s), a host
 * the policy admits; and credentials, backslashes and control characters are
 * refused outright.
 */

/** Protocols an absolute link may use when the policy does not narrow them. */
export const DEFAULT_ALLOWED_LINK_PROTOCOLS = ["http:", "https:", "mailto:"] as const;

/** Internal base used to resolve relative hrefs; never rendered. */
const INTERNAL_BASE = "https://internal.invalid/";

export interface HrefPolicy {
  /** Allowed absolute protocols (with the trailing colon). Default: http, https, mailto. */
  readonly protocols?: readonly string[];
  /**
   * Hosts an absolute http(s) link may target (exact or subdomain). When
   * omitted, any host is allowed for http(s); pass `[]` to allow none, i.e.
   * same-origin/relative links only.
   */
  readonly allowedHosts?: readonly string[];
  readonly maxLength?: number;
  /** Allow `mailto:` even when `protocols` is not given. Default true. */
  readonly allowMailto?: boolean;
}

// eslint-disable-next-line no-control-regex -- control characters are exactly what we refuse
const CONTROL_OR_WS = /[\u0000-\u0020\u007f\u00a0\u2028\u2029\ufeff]/;
const SCHEME = /^[a-z][a-z0-9+.-]*:/i;

export type HrefVerdict =
  | { readonly ok: true; readonly kind: "relative"; readonly href: string }
  | { readonly ok: true; readonly kind: "absolute"; readonly href: string; readonly url: URL }
  | { readonly ok: false; readonly reason: string };

/** Full classification; `isSafeHref` is the boolean view. */
export function classifyHref(href: unknown, policy: HrefPolicy = {}): HrefVerdict {
  if (typeof href !== "string") return { ok: false, reason: "not-a-string" };
  const max = policy.maxLength ?? 2_048;
  if (href.length === 0) return { ok: false, reason: "empty" };
  if (href.length > max) return { ok: false, reason: "too-long" };
  // Whitespace anywhere (leading, trailing, inner) and control characters are
  // refused rather than trimmed: the parser would strip some of them and a
  // renderer might not, so there would be two readings of one string.
  if (CONTROL_OR_WS.test(href)) return { ok: false, reason: "control-or-whitespace" };
  // Backslashes are path separators to the parser (`/\evil` → `//evil`) and
  // nothing a data link legitimately needs.
  if (href.includes("\\")) return { ok: false, reason: "backslash" };

  const absolute = SCHEME.test(href);
  if (!absolute) {
    if (href.startsWith("//")) return { ok: false, reason: "scheme-relative" };
    let url: URL;
    try {
      url = new URL(href, INTERNAL_BASE);
    } catch {
      return { ok: false, reason: "unparsable" };
    }
    // The decisive check: after normalisation the link must still be ours.
    if (url.origin !== new URL(INTERNAL_BASE).origin)
      return { ok: false, reason: "escapes-origin" };
    if (url.username || url.password) return { ok: false, reason: "credentials" };
    return { ok: true, kind: "relative", href };
  }

  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return { ok: false, reason: "unparsable" };
  }
  const protocols =
    policy.protocols ??
    DEFAULT_ALLOWED_LINK_PROTOCOLS.filter((p) => p !== "mailto:" || (policy.allowMailto ?? true));
  if (!protocols.includes(url.protocol)) return { ok: false, reason: `protocol:${url.protocol}` };
  if (url.username || url.password) return { ok: false, reason: "credentials" };
  if (url.protocol === "http:" || url.protocol === "https:") {
    if (url.hostname.length === 0) return { ok: false, reason: "no-host" };
    // The authority must be spelled canonically in the input (`scheme://host`):
    // the parser "repairs" `http:///x`, `http:\\x` and friends into a host the
    // author never wrote; a link whose destination changes under normalisation
    // is refused rather than trusted. (Unicode hosts fail this on purpose —
    // data links must be pre-encoded.)
    const canonicalPrefix = `${url.protocol}//${url.host}`;
    if (!href.toLowerCase().startsWith(canonicalPrefix.toLowerCase()))
      return { ok: false, reason: "non-canonical-authority" };
    if (policy.allowedHosts) {
      const host = url.hostname.toLowerCase();
      const allowed = policy.allowedHosts.some((h) => {
        const a = h.toLowerCase();
        return host === a || host.endsWith(`.${a}`);
      });
      if (!allowed) return { ok: false, reason: `host:${host}` };
    }
  }
  return { ok: true, kind: "absolute", href, url };
}

/**
 * `true` when `href` is a same-origin relative path or an absolute URL with an
 * allowed protocol (and host, when the policy lists any). Rejects
 * `javascript:`, `data:`, `vbscript:`, `file:`, `blob:`, scheme-relative
 * `//…`, backslash and whitespace tricks, credentials and control characters.
 */
export function isSafeHref(href: unknown, policy: HrefPolicy = {}): href is string {
  return classifyHref(href, policy).ok;
}

/**
 * Media (img/video) is stricter than links: same-origin relative paths, or
 * absolute `https:` (host allow-list when given). `data:`, `blob:` and `http:`
 * are never accepted — an image URL in a row is not a licence to inline bytes
 * or to downgrade the page.
 */
export function isSafeMediaSrc(src: unknown, allowedHosts?: readonly string[]): src is string {
  return classifyHref(src, {
    protocols: ["https:"],
    ...(allowedHosts ? { allowedHosts } : {}),
  }).ok;
}
