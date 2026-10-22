// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/scope.ts. Local edits: Prettier formatting.
/**
 * Which scope the home shows (plans/feat-agent-centric.md PR2 "URL rule", R2-12; eng `resolveScope`). Pure.
 *
 * - An explicit `?scope=` always wins.
 * - Else the user's last choice on this browser (stored per user under the drafts prefix, so sign-out clears it).
 * - Else the persona's default, once its read settles; until then nothing is chosen and the page shows neutral
 *   skeleton rows (never a "you own nothing" state that turns into Fleet a moment later).
 * - A persona default of Fleet replaces the URL with `?scope=fleet` once, and never after the user changed any search
 *   param: a user's choice is never overwritten.
 */
import { DRAFT_PREFIX } from "@nasiko/app-core/draftKeys";
import type { Scope } from "./search";

export interface ScopeInput {
  url: Scope | undefined;
  stored: Scope | null;
  /** The persona's default, or null while its read is pending. */
  persona: Scope | null;
  /** The user changed a search param on this visit. */
  interacted: boolean;
}

export function resolveScope(i: ScopeInput): { scope: Scope | null; replace: Scope | null } {
  if (i.url) return { scope: i.url, replace: null };
  if (i.stored) return { scope: i.stored, replace: null };
  if (!i.persona) return { scope: null, replace: null };
  return { scope: i.persona, replace: i.persona === "fleet" && !i.interacted ? "fleet" : null };
}

const key = (sub: string) => `${DRAFT_PREFIX}${sub}:home:scope`;

export function storedScope(sub: string | undefined): Scope | null {
  if (!sub) return null;
  try {
    const v = localStorage.getItem(key(sub));
    return v === "yours" || v === "fleet" ? v : null;
  } catch {
    return null;
  }
}

/** Yours and Fleet are remembered; an org level isn't (the unit may be gone, or no longer visible, next time). */
export function rememberScope(sub: string, scope: Scope) {
  if (scope !== "yours" && scope !== "fleet") return;
  try {
    localStorage.setItem(key(sub), scope);
  } catch {
    // Blocked storage: the choice lasts for this visit (it's in the URL).
  }
}
