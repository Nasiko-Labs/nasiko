// Ported from nasiko-ui-lab@1b99bdda common/src/lib/api/auth.ts. Local edits: `meQuery` (a
// `queryOptions` object under the bare key `['me']`) became `fetchMe(signal)` plus `meQueryOptions(scope)`,
// a plain options object under a scoped product key (React and @tanstack/react-query stay out of this
// module); `login`/`logout` take an options object (`timeout`, `signal`); Prettier formatting.

import * as z from "zod/mini";
import { apiFetch } from "./client";
import { productKey, type ProductKey } from "./keys";
import type { Scope } from "../canonical";

/** The caller's claims from GET /api/me (nasiko-server `Claims`). App-wide, not feature-specific. */
export interface Me {
  sub: string;
  username: string;
  is_superuser: boolean;
}

export interface CallOptions {
  /** ms; 0 turns the deadline off (see apiFetch). */
  timeout?: number | undefined;
  signal?: AbortSignal | undefined;
}

function callInit(o: CallOptions): { timeout?: number; signal?: AbortSignal } {
  return {
    ...(o.timeout === undefined ? {} : { timeout: o.timeout }),
    ...(o.signal === undefined ? {} : { signal: o.signal }),
  };
}

/** GET /api/me — returns the caller's claims ({sub, username, is_superuser}); 401 when not logged in. */
export function fetchMe(o: CallOptions = {}): Promise<Me> {
  return apiFetch<Me>("/api/me", {
    ...callInit(o),
    schema: z.looseObject({ sub: z.string(), username: z.string(), is_superuser: z.boolean() }),
  });
}

export const ME_FEATURE = "me";

/**
 * Query options for the caller's claims. The app's route guard awaits it on every entry: a down
 * server must show its page now, not after retries (`retry: false`). The query cache's 401 handler
 * skips this key while it has no data (no session yet is not an expiry).
 */
export function meQueryOptions(scope: Scope): {
  readonly queryKey: ProductKey;
  readonly queryFn: (ctx: { signal: AbortSignal }) => Promise<Me>;
  readonly retry: false;
  readonly staleTime: number;
} {
  return {
    queryKey: productKey(scope, ME_FEATURE),
    queryFn: ({ signal }) => fetchMe({ signal }),
    retry: false,
    staleTime: 60_000,
  };
}

export interface LoginResult {
  token: string;
  user_id: string;
  username: string;
  is_superuser: boolean;
  expires_in: number;
}

/** POST /api/auth/login — sets the HttpOnly access_token cookie (same-origin). */
export function login(
  username: string,
  password: string,
  o: CallOptions = {},
): Promise<LoginResult> {
  return apiFetch<LoginResult>("/api/auth/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ username, password }),
    ...callInit(o),
  });
}

/**
 * Only same-app relative paths may be used as a post-login redirect target.
 * Rejects absolute URLs, protocol-relative `//host`, backslash tricks and any
 * /login variant (which would loop).
 */
export function safeRedirect(target: unknown, fallback = "/"): string {
  if (typeof target !== "string") return fallback;
  if (!target.startsWith("/") || target.startsWith("//") || target.includes("\\")) return fallback;
  if (/^\/login(?=$|[/?#])/.test(target)) return fallback;
  return target;
}

/** POST /api/auth/logout — clears the login cookie (nasiko-server `auth/login.rs`). */
export function logout(o: CallOptions = {}): Promise<unknown> {
  return apiFetch<unknown>("/api/auth/logout", { method: "POST", ...callInit(o) });
}

/**
 * Minutes until a locked account can sign in again, or null when a 429 is the plain rate limit. nasiko-server
 * (auth/service.rs, ea233d20) locks an account for 15 minutes after 3 failed sign-ins: 429 `account_locked`.
 */
export function lockedFor(body: unknown): number | null {
  if (!body || typeof body !== "object" || (body as { code?: unknown }).code !== "account_locked")
    return null;
  const secs = Number((body as { retry_after_secs?: unknown }).retry_after_secs);
  return Number.isFinite(secs) && secs > 0 ? Math.max(1, Math.ceil(secs / 60)) : 15;
}
