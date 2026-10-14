import * as z from "zod";

/**
 * Delegated-credential contracts (ADR-0001a). These describe what the Rust
 * side issues and validates; the TypeScript side only carries them.
 */
export const DELEGATION_SCOPES = [
  "me:read",
  "usage:read",
  "finops:read",
  "agents:read",
  "surfaces:read",
] as const;
export const DelegationScopeSchema = z.enum(DELEGATION_SCOPES);
export type DelegationScope = z.infer<typeof DelegationScopeSchema>;

/** Max lifetime the CP will ever grant; the client may ask for less, never more. */
export const DELEGATION_MAX_TTL_SECONDS = 60;

/** `POST /api/auth/delegate` request body (workspace mode and central mode). */
export const DelegateRequestSchema = z
  .object({
    scopes: z.array(DelegationScopeSchema).min(1),
    ttl_secs: z.number().int().min(5).max(DELEGATION_MAX_TTL_SECONDS).optional(),
  })
  .strict();
export type DelegateRequest = z.infer<typeof DelegateRequestSchema>;

/** `POST /api/auth/delegate` response body. The token is opaque to clients. */
export const DelegateResponseSchema = z
  .object({
    token: z.string().min(16),
    /** Seconds until expiry, as computed by the issuer. */
    expires_in: z.number().int().positive().max(DELEGATION_MAX_TTL_SECONDS),
    scopes: z.array(DelegationScopeSchema),
    /** The audience the token was issued for (the CP's own). */
    aud: z.string().min(1),
  })
  .strict();
export type DelegateResponse = z.infer<typeof DelegateResponseSchema>;

/** `POST {BFF}/api/delegation/assert` request (central mode): which entitled workspace. */
export const AssertRequestSchema = z.object({ workspace_id: z.uuid() }).strict();
export type AssertRequest = z.infer<typeof AssertRequestSchema>;

/**
 * `POST {BFF}/api/delegation/assert` response (mirrors the EE delegation assertion response). `workspace_url` is the destination resolved by the BFF
 * from its own records — the only place a workspace URL may come from — and
 * is also the assertion's `aud`.
 */
export const AssertResponseSchema = z
  .object({
    assertion: z.string().min(16),
    workspace_url: z.url(),
    expires_in: z.number().int().positive().max(DELEGATION_MAX_TTL_SECONDS),
  })
  .strict();
export type AssertResponse = z.infer<typeof AssertResponseSchema>;

/**
 * A delegated credential as a server-side holder keeps it (request-scoped memory; the Next holder was retired
 * 2026-10-13 and the planned central gateway is Rust). Never
 * serialized to the client. `expiresAt` is a wall-clock ms timestamp.
 */
export interface DelegatedCredential {
  readonly token: string;
  readonly scopes: readonly DelegationScope[];
  readonly aud: string;
  readonly expiresAt: number;
  /** Absolute origin the token is valid for; requests to any other origin are refused. */
  readonly upstream: string;
}
