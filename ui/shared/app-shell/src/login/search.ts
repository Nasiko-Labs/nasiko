// Ported from nasiko-ui-lab@1b99bdda common/src/routes/login.tsx (the route's `validateSearch` schema). Local edits:
// its own module, imported by the server page (a schema exported from the client page module would reach the
// server as a client reference, not a schema); `opt` from `@/lib/search`; Prettier formatting.
import * as z from "zod/mini";
import { opt } from "@nasiko/app-core/search";

/** The page's query: junk falls back to "not set". */
export const loginSearchSchema = z.object({
  redirect: opt(z.string()),
  expired: opt(z.union([z.boolean(), z.string()])),
  // Set by sign out when the logout call failed or timed out (plans/feat-app-shell.md eng D6).
  signout: opt(z.literal("failed")),
  // Set by Settings when a password change came back without a new session (plans/feat-settings.md §1.1).
  password: opt(z.literal("changed")),
});
export type LoginSearch = z.infer<typeof loginSearchSchema>;
