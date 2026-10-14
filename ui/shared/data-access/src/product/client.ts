// Ported from nasiko-ui-lab@1b99bdda common/src/lib/api/client.ts. Local edits: `ApiError` renamed
// `ProductApiError`; `configureProductApi({ origin, fetchImpl })` replaces the page-origin-only URL
// base and the global `fetch` for tests and the harness; `import.meta.env.DEV` became
// `process.env.NODE_ENV !== "production"` (Vite inlines it); `WireSubset` fields admit `undefined`
// (`exactOptionalPropertyTypes`); headers built with `Headers`; Prettier formatting.

/**
 * The product REST client: a thin fetch wrapper for the control plane's `/api/*` and `/health`.
 *
 * Why this does not reuse the (private, Enterprise) generated-surface transport: that transport
 * redacts response bodies by design (contract §4.5: a bounded message and a request id, never
 * the body). Authored screens render the server's own messages — finops 4xx/5xx are often plain
 * text, login answers `{code: "account_locked", retry_after_secs}`, a 404 body decides whether an
 * endpoint is absent (detect.ts) — exactly as the lab does. So this client keeps bodies and
 * exposes them through `ProductApiError`. Folding the two onto one transport with a per-call
 * "keep body" option is a listed follow-up; until then this file is the lab's client verbatim.
 *
 * Requests are same-origin: in development the Vite dev server proxies `/api/*` and `/health` to
 * the control plane (shared/config/vite.config.ts); in production the Rust control plane serves
 * the SPA and the API on one origin (ADR-0008), so the HttpOnly `access_token` cookie set by
 * POST /api/auth/login rides along automatically. No token handling here.
 *
 * Responses are cast to `T` unless the call passes a `schema` (zod or zod/mini, any Standard Schema): then a body that fails it
 * throws a ProductApiError, so the page shows its error state instead of rendering partial data.
 */

export class ProductApiError extends Error {
  readonly status: number;
  readonly body: unknown;
  readonly path: string;
  /** Seconds from the response's `Retry-After` header (429/503), when it sent one. */
  readonly retryAfterSeconds: number | null;

  constructor(
    status: number,
    body: unknown,
    path: string,
    message: string,
    retryAfterSeconds: number | null = null,
  ) {
    super(message);
    this.name = "ProductApiError";
    this.status = status;
    this.body = body;
    this.path = path;
    this.retryAfterSeconds = retryAfterSeconds;
  }

  /** A 403 is never session loss (only a 401 is): render an inline "No access" state. */
  get isForbidden(): boolean {
    return this.status === 403;
  }

  /** Worth retrying: the server (or the proxy in front of it) failed, timed out or rate-limited. Never another 4xx. */
  get isRetryable(): boolean {
    return this.status >= 500 || this.status === 408 || this.status === 429;
  }

  /** The server's human-readable reason, when it sent one (finops 4xx/5xx are often plain text). */
  get serverMessage(): string | null {
    if (typeof this.body === "string" && this.body.trim()) return this.body.trim();
    if (this.body && typeof this.body === "object") {
      const b = this.body as { message?: unknown; error?: unknown };
      if (typeof b.message === "string") return b.message;
      if (typeof b.error === "string") return b.error;
      // JSON-RPC errors (POST /api/orchestrator/a2a): `{error: {code, message}}`.
      if (
        b.error &&
        typeof b.error === "object" &&
        typeof (b.error as { message?: unknown }).message === "string"
      )
        return (b.error as { message: string }).message;
    }
    return null;
  }

  /**
   * The dev rewrite (or the ingress) answers 502/504 when the control plane is not running, and
   * apiFetch maps a network-level fetch failure to 502 too. A "server down" page keys on this.
   */
  get isServerUnreachable(): boolean {
    return this.status === 502 || this.status === 504;
  }
}

/** By name, not `instanceof DOMException`: an error from another realm (jsdom, workers) fails instanceof. */
export const isAbortError = (err: unknown): boolean =>
  (err as { name?: unknown } | null)?.name === "AbortError";
/** A deadline firing: apiFetch's own, or a caller's `AbortSignal.timeout` (same by-name rule as isAbortError). */
export const isTimeoutError = (err: unknown): boolean =>
  (err as { name?: unknown } | null)?.name === "TimeoutError";

/** Retry-worthy: a retryable ProductApiError (never a 401, which is session loss) or a deadline. Aborts never are. */
export const isRetryable = (err: unknown): boolean =>
  (err instanceof ProductApiError && err.status !== 401 && err.isRetryable) || isTimeoutError(err);

/** Every call's deadline unless it passes its own `timeout` (streams don't use apiFetch). */
const API_TIMEOUT_MS = 30_000;

export interface ApiInit extends RequestInit {
  /** ms; 0 turns the deadline off. It combines with the caller's `signal`, and fires as a `TimeoutError`. */
  timeout?: number;
}

/**
 * `T` with every field optional, all the way down: a schema checks only the fields the UI reads.
 * (`| undefined` on each field: under `exactOptionalPropertyTypes` a zod `.optional()` output is
 * `X | undefined`, which the lab's `?: X` form would reject.)
 */
export type WireSubset<T> = T extends readonly (infer U)[]
  ? WireSubset<U>[]
  : T extends object
    ? { [K in keyof T]?: WireSubset<T[K]> | undefined }
    : T;

/**
 * `schema` checks the body (apiData: its `data`). Its fields must agree with `T` (a compile error otherwise), and it
 * should be `z.looseObject` all the way down, so a field the server adds never fails. It checks and never
 * transforms: the body is returned as the server sent it.
 */
export interface Checked<T> {
  schema?: WireSchema<WireSubset<T>> | undefined;
}

export interface ProductApiConfig {
  /**
   * Base the paths resolve against. Default: the page origin (`globalThis.location.origin`), or
   * `http://localhost` where there is none (Node's fetch rejects relative URLs). Tests and the
   * harness point it at their own server.
   */
  origin?: string | undefined;
  /** The fetch to use; default `globalThis.fetch`, read per call so a test may stub it. */
  fetchImpl?: typeof fetch | undefined;
}

let config: ProductApiConfig = {};

/** Module-level override for tests and the harness; `configureProductApi({})` restores the defaults. */
export function configureProductApi(next: ProductApiConfig): void {
  config = { ...next };
}

function baseOrigin(): string {
  // `location` is absent in Node (tests, the server); the DOM lib types it as always present.
  const page = (globalThis as { location?: { origin: string } }).location;
  return config.origin ?? page?.origin ?? "http://localhost";
}

const isDev = (): boolean =>
  typeof process !== "undefined" && process.env.NODE_ENV !== "production";

/**
 * Any Standard Schema (https://standardschema.dev): zod's classic schemas and `zod/mini`'s both implement it, so the
 * app shell can check its reads with `zod/mini` and stay out of classic zod's bundle (check:bundles), while feature
 * routes keep the lab's classic schemas. Only synchronous validation is supported (zod's is synchronous).
 */
export interface WireSchema<Out> {
  readonly "~standard": {
    readonly validate: (value: unknown) => WireResult<Out> | Promise<WireResult<Out>>;
  };
}
type WireResult<Out> =
  | { readonly value: Out; readonly issues?: undefined }
  | {
      readonly issues: readonly {
        readonly message: string;
        readonly path?: readonly (PropertyKey | { readonly key: PropertyKey })[] | undefined;
      }[];
    };

/** A body that fails its schema is a ProductApiError (status 200, never retried); dev builds log where it failed. */
function check(schema: WireSchema<unknown>, value: unknown, path: string, method = "GET"): void {
  const r = schema["~standard"].validate(value);
  if (r instanceof Promise)
    throw new Error(`${method} ${path}: asynchronous schemas are not supported`);
  if (!r.issues) return;
  const where = r.issues
    .slice(0, 5)
    .map(
      (i) =>
        `${(i.path ?? []).map((k) => String(typeof k === "object" ? k.key : k)).join(".") || "$"}: ${i.message}`,
    );
  if (isDev()) console.error(`${method} ${path}: the response failed its schema`, where);
  throw new ProductApiError(
    200,
    value,
    path,
    `${method} ${path} → unexpected response (${where[0] ?? "schema"})`,
  );
}

export async function apiFetch<T>(path: string, init: ApiInit & Checked<T> = {}): Promise<T> {
  const { timeout = API_TIMEOUT_MS, signal, schema, ...rest } = init;
  const signals = [signal, timeout ? AbortSignal.timeout(timeout) : null].filter(
    (s): s is AbortSignal => !!s,
  );
  let res: Response;
  try {
    // Absolute against the page origin: identical in the browser, and required by
    // Node's fetch under Vitest (it rejects relative URLs).
    const doFetch = config.fetchImpl ?? globalThis.fetch;
    // The lab spreads `{Accept, ...rest.headers}`; a Headers object does the same without
    // misreading a `Headers` instance or a tuple list (caller headers still win).
    const headers = new Headers(rest.headers);
    if (!headers.has("accept")) headers.set("accept", "application/json");
    const reqInit: RequestInit = { ...rest, credentials: "same-origin", headers };
    const merged = signals.length > 1 ? AbortSignal.any(signals) : signals[0];
    if (merged) reqInit.signal = merged;
    res = await doFetch(new URL(path, baseOrigin()), reqInit);
  } catch (err) {
    // Aborts and timeouts are rethrown, so the caller can say "cancelled" or "slow", not "down".
    if (isAbortError(err) || isTimeoutError(err)) throw err;
    throw new ProductApiError(502, null, path, `${rest.method ?? "GET"} ${path} → network error`);
  }
  const text = await res.text();
  const body: unknown = text ? safeJson(text) : null;
  if (!res.ok)
    throw new ProductApiError(
      res.status,
      body,
      path,
      `${rest.method ?? "GET"} ${path} → ${res.status}`,
      retryAfter(res.headers.get("retry-after")),
    );
  if (schema) check(schema, body, path, rest.method);
  return body as T;
}

/** `Retry-After` in seconds (delta form; an HTTP date counts from now). */
function retryAfter(raw: string | null): number | null {
  if (!raw) return null;
  const secs = /^\d+$/.test(raw.trim()) ? Number(raw) : (Date.parse(raw) - Date.now()) / 1000;
  return Number.isFinite(secs) && secs > 0 ? Math.ceil(secs) : null;
}

/** For endpoints that wrap their payload in `{data, status_code, message}`. */
export async function apiData<T>(path: string, init: ApiInit & Checked<T> = {}): Promise<T> {
  const { schema, ...rest } = init;
  const body = await apiFetch<unknown>(path, rest);
  // An empty 204, an HTML fallback page or a changed wire format must fail loudly,
  // not turn into `undefined` cast to T.
  if (!body || typeof body !== "object" || !("data" in body) || body.data == null) {
    throw new ProductApiError(
      200,
      body,
      path,
      `${init.method ?? "GET"} ${path} → unexpected response (no data envelope)`,
    );
  }
  const data: unknown = body.data;
  if (schema) check(schema, data, path, init.method);
  return data as T;
}

/** Build `path?k=v` from a params object, dropping undefined/null/empty values. */
export function withQuery(
  path: string,
  params: Record<string, string | number | boolean | null | undefined>,
): string {
  const qs = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === "") continue;
    qs.set(k, String(v));
  }
  const s = qs.toString();
  return s ? `${path}?${s}` : path;
}

export function safeJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}
