// Development (5d46c313f): `isPreview`.
/** Public build configuration. VITE_* values ship to every browser: never put credentials here.
 * APIs stay same-origin; NASIKO_API_UPSTREAM configures only the local development proxy.
 * Test mocks require an explicit development/test build and are excluded from releases.
 */
type ApiMode = "mock" | "live";

/**
 * The lab's selective-mock keys (`VITE_NASIKO_MOCK`) are a Vite dev-server feature with no counterpart here: the
 * mock worker answers everything or nothing (`VITE_NASIKO_API_MODE`). The names stay so callers compile.
 */
export const MOCKABLE = [] as const;
export type Mockable = (typeof MOCKABLE)[number];

export interface EnvConfig {
  mode: ApiMode;
  /** Endpoints mocked while in live mode. Always empty here (see `MOCKABLE`). */
  partialMocks: Mockable[];
  legacyUiUrl: string | null;
  /** The waitlist page, tagged `ref=oss-app`; null hides every waitlist link. */
  waitlistUrl: string | null;
}

export interface RawEnv {
  VITE_NASIKO_API_MODE?: string | undefined;
  VITE_NASIKO_ALLOW_MOCK_BUILD?: string | undefined;
  VITE_NASIKO_LEGACY_UI_URL?: string | undefined;
  VITE_NASIKO_WAITLIST_URL?: string | undefined;
  DEV?: boolean | undefined;
  PROD?: boolean | undefined;
}

/** Vite replaces these public values at build time. */
function publicEnv(): RawEnv {
  return {
    VITE_NASIKO_API_MODE: import.meta.env.VITE_NASIKO_API_MODE as string | undefined,
    VITE_NASIKO_ALLOW_MOCK_BUILD: import.meta.env.VITE_NASIKO_ALLOW_MOCK_BUILD as
      string | undefined,
    VITE_NASIKO_LEGACY_UI_URL: import.meta.env.VITE_NASIKO_LEGACY_UI_URL as string | undefined,
    VITE_NASIKO_WAITLIST_URL: import.meta.env.VITE_NASIKO_WAITLIST_URL as string | undefined,
    DEV: import.meta.env.DEV,
    PROD: import.meta.env.PROD,
  };
}

export function readEnv(
  env: RawEnv = publicEnv(),
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- undefined on the server, whatever lib.dom says
  origin: string | undefined = globalThis.location?.origin,
): EnvConfig {
  const rawMode = (env.VITE_NASIKO_API_MODE ?? "").trim();
  let mode: ApiMode =
    rawMode === "live" || rawMode === "mock" ? rawMode : env.DEV ? "mock" : "live";
  // A production build never serves fabricated data by accident: mock mode there needs an explicit opt-in.
  const mockAllowed = !env.PROD || (env.VITE_NASIKO_ALLOW_MOCK_BUILD ?? "") === "true";
  if (mode === "mock" && !mockAllowed) mode = "live";

  // Dev defaults to the local OSS server; a production build defaults to its own origin
  // (the nasiko server serves both the API and the legacy UI).
  const fallbackUi = env.DEV ? "http://localhost:8080" : origin;
  return {
    mode,
    partialMocks: [],
    legacyUiUrl: validHttpUrl(env.VITE_NASIKO_LEGACY_UI_URL || fallbackUi || ""),
    waitlistUrl: waitlistHref(env.VITE_NASIKO_WAITLIST_URL || WAITLIST_URL),
  };
}

/** The Nasiko waitlist page, in dev servers and builds alike, unless VITE_NASIKO_WAITLIST_URL names another. */
const WAITLIST_URL = "https://nasiko-waitlist.vercel.app";

/** An absolute http(s) URL with `ref=oss-app` added (its own query and hash kept), so the page can count sign-ups. */
function waitlistHref(value: string): string | null {
  const v = value.trim();
  if (!/^https?:\/\//i.test(v)) return null;
  try {
    const u = new URL(v);
    u.searchParams.set("ref", "oss-app");
    return u.href;
  } catch {
    return null;
  }
}

/** Accept only absolute http(s) URLs; anything else disables trace links rather than risk a bad href. */
export function validHttpUrl(value: string): string | null {
  const v = value.trim();
  if (!/^https?:\/\//i.test(v)) return null;
  try {
    const u = new URL(v);
    return u.origin + u.pathname.replace(/\/$/, "");
  } catch {
    return null;
  }
}

export const env = readEnv();

/**
 * A preview section may render (development 5d46c313f; plans/feat-agent-centric.md eng, preview policy): true only in
 * full mock mode or when the feature's group is partially mocked. It gates the section's queries and rendering, whatever
 * the server answers, so a preview never draws from an unexpected live response. Partial mocks are a Vite dev-server
 * feature this build has none of (`MOCKABLE`), so here it is mock mode alone; the group stays for the callers.
 */
export function isPreview(
  group: string,
  config: Pick<EnvConfig, "mode" | "partialMocks"> = env,
): boolean {
  return config.mode === "mock" || (config.partialMocks as readonly string[]).includes(group);
}
