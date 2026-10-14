"use client";

/**
 * Mode (System / Light / Dark, `theme` here) and colour theme (Teal / Indigo / Plum / Carbon, `accent` here), per
 * viewer. Ported near-verbatim from the lab's `common/src/app/shell/theme.ts` (nasiko-ui-lab 1b99bdda); the only
 * additions are `forceTheme` (the stories' and tests' forced mode) and the labels living here instead of a copy file.
 *
 * Stored in localStorage under THEME_KEY / ACCENT_KEY. A stored value from the retired accent presets
 * (violet, slate, gold) reads as Carbon; `indigo` keeps its name. The pre-paint script (`prepaint.ts`) reads the
 * same keys before first paint (it can't import this file, so the names are duplicated there and
 * `theme-store.test.tsx` checks they match). Every storage access is guarded: with storage blocked the
 * defaults apply and choices last for the page's lifetime.
 *
 * A vanilla zustand store (written outside React by the storage and matchMedia listeners) with
 * `persist`, whose storage maps the two fields onto the two raw keys the pre-paint script reads.
 */
import { createStore, useStore } from "zustand";
import { persist, type PersistStorage } from "zustand/middleware";

export const THEME_KEY = "openruntime.theme";
export const ACCENT_KEY = "openruntime.accent";

export const THEMES = [
  { id: "system", label: "System" },
  { id: "light", label: "Light" },
  { id: "dark", label: "Dark" },
] as const;
export type Theme = (typeof THEMES)[number]["id"];

/**
 * Swatch colours are each theme's light `--primary` (checked against tokens.css by css.test.ts),
 * shown beside the name so the choice never relies on colour alone.
 */
export const ACCENTS = [
  { id: "teal", label: "Teal", swatch: "#006375" },
  { id: "indigo", label: "Indigo", swatch: "#4849a9" },
  { id: "plum", label: "Plum", swatch: "#853867" },
  // shadcn's default neutral: Mist in light mode, Carbon in dark.
  { id: "carbon", label: "Carbon", swatch: "oklch(0.205 0 0)" },
] as const;
export type Accent = (typeof ACCENTS)[number]["id"];

const DEFAULT_THEME: Theme = "system";
const DEFAULT_ACCENT: Accent = "carbon";

/** The sign-in screen is always Carbon, whatever the stored choice (user decision 2026-09-30, beside Aceternity's
 *  black showcase and the gold mark); mode still follows the choice. The pre-paint script repeats this. */
export const LOGIN_ACCENT: Accent = "carbon";
export const pinnedAccentFor = (pathname: string): Accent | null =>
  /\/login\/?$/.test(pathname) ? LOGIN_ACCENT : null;
let pinnedAccent: Accent | null = null;

/** A mode forced by a story or a test (`UiProvider forcedTheme`): applied, never stored. */
export type ForcedTheme = Exclude<Theme, "system">;
let forcedTheme: ForcedTheme | null = null;

const DARK_QUERY = "(prefers-color-scheme: dark)";

const isTheme = (v: unknown): v is Theme => THEMES.some((t) => t.id === v);
const isAccent = (v: unknown): v is Accent => ACCENTS.some((a) => a.id === v);

function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string) {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Storage blocked: the choice still applies until the page closes.
  }
}

export interface ThemePrefs {
  theme: Theme;
  accent: Accent;
}

/** Stored choices; unknown or missing values fall back to System and Carbon. */
export function readPrefs(): ThemePrefs {
  const theme = read(THEME_KEY);
  const accent = read(ACCENT_KEY);
  return {
    theme: isTheme(theme) ? theme : DEFAULT_THEME,
    accent: isAccent(accent) ? accent : DEFAULT_ACCENT,
  };
}

function systemIsDark(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia(DARK_QUERY).matches;
}

/**
 * Sets `.dark` and `data-theme` on <html>, exactly as the pre-paint script does. Colours never
 * animate on a switch: transitions are off for the frame the tokens change in, so buttons
 * with `transition-all` don't fade while the rest of the page snaps.
 */
export function applyTheme(prefs: ThemePrefs, root: HTMLElement = document.documentElement) {
  const mode = forcedTheme ?? prefs.theme;
  const dark = mode === "dark" || (mode === "system" && systemIsDark());
  const accent = pinnedAccent ?? prefs.accent;
  const changed =
    root.classList.contains("dark") !== dark || root.getAttribute(THEME_ATTR) !== accent;
  if (changed) root.classList.add(THEME_SWITCHING_CLASS);
  root.classList.toggle("dark", dark);
  root.setAttribute(THEME_ATTR, accent);
  if (!changed) return;
  // Force the new styles to apply with transitions off, then turn them back on.
  // Reading the layout is the point: it forces the style recalculation with transitions off.
  // eslint-disable-next-line @typescript-eslint/no-unused-expressions -- forced reflow
  root.offsetWidth;
  const done = () => {
    root.classList.remove(THEME_SWITCHING_CLASS);
  };
  if (typeof requestAnimationFrame === "function") requestAnimationFrame(done);
  else done();
}

/** The <html> attribute that selects the colour theme (tokens.css `[data-theme]` blocks). */
const THEME_ATTR = "data-theme";

/** On <html> while tokens change; tokens.css turns every transition off under it. */
export const THEME_SWITCHING_CLASS = "theme-switching";

/**
 * Not JSON under one key: each field is its own raw string, as the pre-paint script reads it. A write stores
 * both fields, so a retired accent still stored is rewritten as Carbon on the next change.
 */
const storage: PersistStorage<ThemePrefs> = {
  getItem: () => ({ state: readPrefs(), version: 0 }),
  setItem: (_name, { state }) => {
    write(THEME_KEY, state.theme);
    write(ACCENT_KEY, state.accent);
  },
  removeItem: () => {},
};

const store = createStore<ThemePrefs>()(
  persist((): ThemePrefs => ({ theme: DEFAULT_THEME, accent: DEFAULT_ACCENT }), {
    name: "openruntime.theme-prefs",
    storage,
    // The store is module-level, so on the server one instance serves every request: it must never read storage
    // there. Without this it still could not (`read()` reaches `window.localStorage`, and the ReferenceError is
    // swallowed), but that rested on a caught exception; now the server simply never hydrates and keeps the
    // defaults, which is also what `useThemePrefs()` renders during hydration (`getInitialState`), so SSR is
    // theme-neutral and request-independent. The client hydrates at import and again in `initTheme()`.
    skipHydration: typeof window === "undefined",
  }),
);

function commit(next: Partial<ThemePrefs>) {
  store.setState(next);
  applyTheme(store.getState());
}

export function setTheme(theme: Theme) {
  commit({ theme });
}

export function setAccent(accent: Accent) {
  commit({ accent });
}

/** Shows one colour theme while a page is open (the sign-in screen); `null` goes back to the stored choice. */
export function pinAccent(accent: Accent | null) {
  pinnedAccent = accent;
  applyTheme(store.getState());
}

/** Shows one mode while a story or test is open, without storing it; `null` goes back to the stored choice. */
export function forceTheme(theme: ForcedTheme | null) {
  forcedTheme = theme;
  applyTheme(store.getState());
}

/**
 * Called once at boot. Applies the stored choice (so the page is right even if the inline
 * pre-paint script was blocked, e.g. by a CSP), follows the OS while the choice is System, and
 * picks up choices made in other tabs. Returns the unsubscribe for tests.
 */
export function initTheme(): () => void {
  // Boot on /login keeps the pre-paint script's Carbon; the page's own pin takes over once it mounts.
  pinnedAccent = pinnedAccentFor(window.location.pathname);
  const load = () => {
    void store.persist.rehydrate(); // synchronous, like its storage
    applyTheme(store.getState());
  };
  load();
  const onStorage = (e: StorageEvent) => {
    if (e.key === THEME_KEY || e.key === ACCENT_KEY || e.key === null) load();
  };
  window.addEventListener("storage", onStorage);
  if (typeof window.matchMedia !== "function")
    return () => {
      window.removeEventListener("storage", onStorage);
    };
  const mql = window.matchMedia(DARK_QUERY);
  const onChange = () => {
    if (store.getState().theme === "system") applyTheme(store.getState());
  };
  mql.addEventListener("change", onChange);
  return () => {
    mql.removeEventListener("change", onChange);
    window.removeEventListener("storage", onStorage);
  };
}

/** Tests: re-read the choice from storage. */
export function resetThemeState() {
  void store.persist.rehydrate();
}

export function useThemePrefs(): ThemePrefs {
  return useStore(store);
}
