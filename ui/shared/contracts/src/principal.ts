import * as z from "zod";

/**
 * The minimal identity DTO the session bootstrap (`GET /api/me`) gives React. Never contains a
 * token. `epoch` changes whenever authorization context may have changed
 * (workspace switch, re-login), and every cache key carries it.
 */
export const PrincipalSchema = z
  .object({
    /** Opaque to the client; the CP owns the format (a UUID in the real CP). */
    userId: z.string().min(1).max(128),
    username: z.string().min(1),
    workspaceId: z.string().min(1),
    /** Monotonic per browser session; bumped on workspace switch / logout. */
    epoch: z.number().int().nonnegative(),
    /** Read-only summary for UI decisions; authority stays in Rust. */
    isSuperuser: z.boolean().default(false),
  })
  .strict();
export type Principal = z.infer<typeof PrincipalSchema>;
