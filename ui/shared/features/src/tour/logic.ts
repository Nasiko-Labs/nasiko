// Ported from the legacy ui/ on development (3aac84f66) common/src/features/tour/logic.ts (dd9fe0f64). Local edits:
// `LayerTour` from @nasiko/app-core/edition; Prettier formatting.
/**
 * Pure tour rules: which tour a page has, and what this browser remembers per user. Import-light: it loads with the
 * shell (TourHost), while the tours' text and the overlay load only when a tour opens (index.ts).
 */
import type { LayerTour } from "@nasiko/app-core/edition/edition";

/**
 * The app tour (the sidebar: it rides along on the Overview's tour and on a first-time user's first tour), then one per
 * page. Ids are storage keys: rename one and its users see it again.
 */
export const TOUR_IDS = [
  "app",
  "overview",
  "chat",
  "agents",
  "agent",
  "deploy",
  "builds",
  "build",
  "workflows",
  "workflow",
  "workflow-new",
  "router",
  "mcp",
  "mcp-server",
  "sessions",
  "session",
  "flows",
  "flow",
  "tokenops",
  "optimization",
  "harnesses",
  "settings",
  "account",
  "status",
] as const;
export type TourId = (typeof TOUR_IDS)[number];

/** First match wins: detail pages before their lists. */
const PAGES: readonly [RegExp, TourId][] = [
  [/^\/$/, "overview"],
  [/^\/chat(\/|$)/, "chat"],
  [/^\/agents\/(?!mine$)[^/]+$/, "agent"],
  [/^\/agents(\/mine)?$/, "agents"],
  [/^\/deploy$/, "deploy"],
  [/^\/builds$/, "builds"],
  [/^\/builds\/[^/]+$/, "build"],
  [/^\/workflows(\/(drafts|runs))?$/, "workflows"],
  [/^\/workflows\/new$/, "workflow-new"],
  [/^\/workflows\/[^/]+$/, "workflow"],
  [/^\/router$/, "router"],
  [/^\/mcp$/, "mcp"],
  [/^\/mcp\/[^/]+$/, "mcp-server"],
  [/^\/sessions$/, "sessions"],
  [/^\/sessions\/[^/]+$/, "session"],
  [/^\/flows$/, "flows"],
  [/^\/flows\/[^/]+$/, "flow"],
  [/^\/tokenops$/, "tokenops"],
  [/^\/optimization$/, "optimization"],
  [/^\/harnesses$/, "harnesses"],
  [/^\/settings(\/|$)/, "settings"],
  [/^\/account(\/|$)/, "account"],
  [/^\/status$/, "status"],
];

/** A layer's tours, as far as these rules need them (the `tours` slot). */
type Layered = readonly Pick<LayerTour, "id" | "match">[];

/** The page's own tour: the core's, else a layer's (EE: Access control), else null (a page with no tour). */
export function tourFor(pathname: string, layers: Layered = []): string | null {
  return (
    PAGES.find(([re]) => re.test(pathname))?.[1] ??
    layers.find((t) => t.match.test(pathname))?.id ??
    null
  );
}

export interface TourPrefs {
  /** Tours start on their own (a first-time user, until they say No thanks). */
  auto: boolean;
  /** Tour ids: the core's and any layer's. */
  seen: readonly string[];
}
export const NO_PREFS: TourPrefs = { auto: false, seen: [] };

export interface Due {
  id: string;
  /** The app tour's steps come first (`tours.ts` `stepsFor`). */
  withApp: boolean;
}

/** The tour to start on its own here, if any: the first one carries the app tour, then each page's own, once. */
export function dueTour(prefs: TourPrefs, pathname: string, layers: Layered = []): Due | null {
  if (!prefs.auto) return null;
  const page = tourFor(pathname, layers);
  if (!prefs.seen.includes("app")) return { id: page ?? "app", withApp: true };
  return page && !prefs.seen.includes(page) ? { id: page, withApp: false } : null;
}

export const markSeen = (prefs: TourPrefs, id: string): TourPrefs =>
  prefs.seen.includes(id) ? prefs : { ...prefs, seen: [...prefs.seen, id] };

/**
 * Not under the drafts prefix: sign-out clears drafts, and a tour seen once stays seen. Per user, so a shared browser
 * still greets the next new user.
 */
export const prefsKey = (sub: string) => `openruntime.tour:${sub}`;

/** Whatever is stored: anything unreadable is no prefs (tours off, nothing seen). */
export function parsePrefs(raw: string | null): TourPrefs {
  if (!raw) return NO_PREFS;
  try {
    const v = JSON.parse(raw) as Partial<TourPrefs>;
    return {
      auto: v.auto === true,
      // Any id: a layer's tours are known only to that edition, and an unknown id costs nothing.
      seen: Array.isArray(v.seen) ? v.seen.filter((s): s is string => typeof s === "string") : [],
    };
  } catch {
    return NO_PREFS;
  }
}
