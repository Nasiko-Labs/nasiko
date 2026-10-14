"use client";

import { LazyMotion, MotionConfig } from "motion/react";
import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useDensity } from "../theme/density";
import { PortalContainerContext } from "./portal";
import {
  clampRecipe,
  resolveMotionMode,
  type EffectiveRecipe,
  type MotionMode,
  type MotionPreference,
  type MotionProfile,
  type MotionRecipe,
} from "./policy";

/**
 * Motion host (contract §8.2, §8.6). Owns the three inputs, resolves the mode,
 * and publishes it two ways at once:
 *  - as `data-motion` / `data-motion-profile` on a wrapper element, so the CSS
 *    tokens clamp every CSS transition — including vendored and extension
 *    code — without JS; and
 *  - through context, so `m.*` recipes read the same decision.
 *
 * SSR: the wrapper attributes render on the server from `preference` (a server-
 * known user setting), and the OS media query is read before the first paint
 * through `useSyncExternalStore` with a server snapshot of `false`; the CSS
 * `prefers-reduced-motion` rule covers that first paint, so a reduced-motion
 * user never sees an initial animated flash.
 *
 * Expressive motion is never a default: `profile` here is the surface ceiling,
 * and a nested `<ExpressiveRegion>` is the only way to raise a subtree to it.
 */

/** Diagnostic event emitted by recipe adapters (harness/test only in practice). */
export interface MotionTraceEvent {
  /** Stable per-instance id (React useId) so repeated callbacks for one element can be grouped. */
  readonly id: string;
  readonly adapter: "reveal" | "presence" | "emphasis";
  readonly phase: "start" | "complete" | "skipped";
  readonly recipe: string;
  readonly effective: EffectiveRecipe;
  readonly at: number;
}

export interface MotionContextValue {
  readonly mode: MotionMode;
  readonly profile: MotionProfile;
  readonly preference: MotionPreference;
  readonly osReduced: boolean;
  /** Whether content entering this surface should animate in at all (false when restoring history). */
  readonly enter: boolean;
  /** Clamp a semantic recipe name (a generator may pass anything; unknown names are instant). */
  readonly recipe: (name: MotionRecipe | (string & {})) => EffectiveRecipe;
  /** Optional diagnostics sink; recipes call it on start/complete/skipped. */
  readonly trace?: ((e: MotionTraceEvent) => void) | undefined;
}

const MotionContext = createContext<MotionContextValue | null>(null);

const QUERY = "(prefers-reduced-motion: reduce)";
function subscribe(cb: () => void): () => void {
  if (typeof window.matchMedia !== "function") return () => undefined;
  const mql = window.matchMedia(QUERY);
  mql.addEventListener("change", cb);
  return () => {
    mql.removeEventListener("change", cb);
  };
}
const getSnapshot = () =>
  typeof window.matchMedia === "function" ? window.matchMedia(QUERY).matches : false;
// Unknown is treated as *reduced*: the server (and the hydration render, which
// must match it) cannot know the OS setting, so the only safe assumption is
// the restrictive one. The store re-reads matchMedia right after hydration and
// a user whose OS allows motion gets `full` from the next render; nothing that
// was already on screen animates retroactively (recipes only animate on mount).
const getServerSnapshot = () => true;

/** OS reduced-motion preference, SSR-safe; `true` until the client has read it. Exported for tests and diagnostics. */
export function useOsReducedMotion(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
}

const noop = () => () => undefined;
/**
 * `false` on the server and during the hydration render, `true` afterwards.
 * Recipes use it so server-rendered content is never emitted with an entrance
 * start state (opacity 0 in the HTML): what the server sends is final, and
 * only elements mounted after hydration animate in.
 */
export function useHydrated(): boolean {
  return useSyncExternalStore(
    noop,
    () => true,
    () => false,
  );
}

const loadFeatures = () => import("./features").then((m) => m.default);
/**
 * The lab's feature set (`common/src/lib/motionFeatures.ts`: `domMax`), opted into by the app scope that hosts
 * ported lab screens: shared-layout (`layoutId`) morphs between pages need the layout features. Its own chunk,
 * loaded after first paint like the default one; `m` components render at rest until then.
 */
const loadMaxFeatures = () => import("./features-max").then((m) => m.default);

const MODE_RANK: Record<MotionMode, number> = { none: 0, reduced: 1, full: 2 };
/** The stricter of two modes. A nested surface can only lower what its parent allows. */
export function clampMode(parent: MotionMode, own: MotionMode): MotionMode {
  return MODE_RANK[own] < MODE_RANK[parent] ? own : parent;
}

/**
 * Ownership (Phase 3 closeout):
 *  - exactly one **application-scope** provider (mounted by `UiProvider`) owns
 *    the application-wide preference and publishes the resolved mode on
 *    `<html>`; its overlays portal to `document.body` and are governed there;
 *  - any number of **surface-scope** providers (a harness host, a Weave
 *    surface, a test) resolve their own profile/preference *clamped by their
 *    parent's mode* — never above the application/OS ceiling — and own a
 *    portal container that carries that surface's effective policy as its own
 *    `data-motion` attributes, so the overlays a surface opens are governed by
 *    the surface, not by the app.
 * No global "last provider wins" state exists; siblings never affect each other.
 *
 * Where the surface container lives (Phase 4): it is a React portal into
 * `document.body`, mounted after hydration. React context still flows to it
 * (it is in the surface's React tree), so overlays keep the surface's motion,
 * theme and density context; and because it is *not* a DOM descendant of the
 * surface, a surface host that scrolls, clips (`overflow`) or establishes a
 * containing block (`transform`, `filter`, `contain`, `will-change`) cannot
 * trap or clip a fixed-position overlay. That closes the containing-block
 * limitation recorded in the Phase 3 closeout; test:motion covers scrolling,
 * transformed and clipped hosts. The policy is carried on the container
 * itself (`data-motion`, `data-motion-profile`, `data-density`), not inherited
 * from an ancestor, so moving it out of the wrapper changes nothing the CSS sees.
 */
let appScopeMounted = 0;
const RawMotionContext = MotionContext;

export interface MotionProviderProps {
  /** `app` for the one application root (UiProvider); `surface` (default) for nested hosts. */
  readonly scope?: "app" | "surface";
  /**
   * Which lazy feature bundle the root `LazyMotion` loads: `animation` (default, `domAnimation`) or `max`
   * (`domMax`, the lab's set: layout animations and drag). Read only by the provider that owns the runtime (the
   * outermost one); a nested provider reuses its ancestor's bundle whatever it asks for.
   */
  readonly features?: "animation" | "max";
  /** Surface ceiling. Default `operational`. Use `none` for print/export/testing baselines. */
  readonly profile?: MotionProfile;
  /** The user's own setting; `system` follows the OS. Server-known, so it renders on the server. */
  readonly preference?: MotionPreference;
  /** `false` when the surface is being restored (history, reload): nothing replays. */
  readonly enter?: boolean;
  readonly children: ReactNode;
  /** Test hook: force the OS value instead of reading matchMedia. */
  readonly osReducedOverride?: boolean;
  /** Diagnostics sink (harness, test:motion, Phase 6 traces). */
  readonly trace?: (e: MotionTraceEvent) => void;
}

export function MotionProvider({
  scope = "surface",
  features = "animation",
  profile = "operational",
  preference = "system",
  enter = true,
  children,
  osReducedOverride,
  trace,
}: MotionProviderProps) {
  const osLive = useOsReducedMotion();
  const osReduced = osReducedOverride ?? osLive;
  const parent = useContext(RawMotionContext);
  const value = useMemo<MotionContextValue>(() => {
    const own = resolveMotionMode({ profile, preference, osReduced });
    // A surface cannot exceed what its parent (ultimately the app/OS) allows.
    const mode = parent && scope === "surface" ? clampMode(parent.mode, own) : own;
    return {
      mode,
      profile,
      preference,
      osReduced,
      enter,
      trace,
      recipe: (name) => clampRecipe(name, mode, profile),
    };
  }, [scope, profile, preference, osReduced, enter, trace, parent]);

  // Application scope: the single owner of html[data-motion].
  useEffect(() => {
    if (scope !== "app") return undefined;
    appScopeMounted += 1;
    if (appScopeMounted > 1 && process.env.NODE_ENV !== "production")
      console.error(
        'MotionProvider: more than one scope="app" provider is mounted; only UiProvider should own the application preference.',
      );
    document.documentElement.dataset.motion = value.mode;
    document.documentElement.dataset.motionProfile = value.profile;
    return () => {
      appScopeMounted -= 1;
      if (appScopeMounted === 0) {
        delete document.documentElement.dataset.motion;
        delete document.documentElement.dataset.motionProfile;
      }
    };
  }, [scope, value.mode, value.profile]);

  // Surface scope: own portal container, portalled to <body> after hydration
  // (never in the server HTML; overlays only open on the client).
  const [container, setContainer] = useState<HTMLElement | null>(null);
  const hydrated = useHydrated();
  const density = useDensity();
  const inner = (
    <div
      data-motion={value.mode}
      data-motion-profile={value.profile}
      style={{ display: "contents" }}
    >
      {children}
      {scope === "surface" && hydrated
        ? createPortal(
            <div
              data-portal-root=""
              data-motion={value.mode}
              data-motion-profile={value.profile}
              data-density={density}
              ref={setContainer}
            />,
            document.body,
          )
        : null}
    </div>
  );
  // The library's own reduced handling is driven by *our* decision, so there is one policy, not two:
  // `reducedMotion="always"` makes positional values (x/y/width/height…) instant below `full`, and at `none`
  // `skipAnimations` makes every value instant (opacity, filter, clip-path too) for every motion element in the
  // subtree, including vendored ones that do not consult the context (the chart kit's axes, grid, markers).
  const config = {
    reducedMotion: value.mode === "full" ? ("never" as const) : ("always" as const),
    skipAnimations: value.mode === "none",
  };
  const body = parent ? (
    // Nested: reuse the ancestor's LazyMotion (one runtime); only the policy changes.
    <MotionConfig {...config}>{inner}</MotionConfig>
  ) : (
    <LazyMotion features={features === "max" ? loadMaxFeatures : loadFeatures} strict>
      <MotionConfig {...config}>{inner}</MotionConfig>
    </LazyMotion>
  );
  return (
    <MotionContext.Provider value={value}>
      <PortalContainerContext.Provider value={scope === "surface" ? container : null}>
        {body}
      </PortalContainerContext.Provider>
    </MotionContext.Provider>
  );
}

/**
 * Raises the ceiling to `expressive` for an admitted presentation context only
 * (a showcase story, a public capability). It cannot raise the user's, the OS's
 * or the parent surface's decision: the mode is re-resolved with the same
 * preference and OS inputs and then clamped by the parent mode.
 */
export function ExpressiveRegion({ children, enter }: { children: ReactNode; enter?: boolean }) {
  const parent = useMotion();
  const value = useMemo<MotionContextValue>(() => {
    const profile: MotionProfile = parent.profile === "none" ? "none" : "expressive";
    const mode = clampMode(
      parent.mode,
      resolveMotionMode({ profile, preference: parent.preference, osReduced: parent.osReduced }),
    );
    return {
      ...parent,
      profile,
      mode,
      enter: enter ?? parent.enter,
      recipe: (name) => clampRecipe(name, mode, profile),
    };
  }, [parent, enter]);
  return (
    <MotionContext.Provider value={value}>
      <div
        data-motion={value.mode}
        data-motion-profile={value.profile}
        style={{ display: "contents" }}
      >
        {children}
      </div>
    </MotionContext.Provider>
  );
}

/** Marks a subtree as restored (history, reload, screenshot): entrances do not replay. */
export function RestoredSurface({ children }: { children: ReactNode }) {
  const parent = useMotion();
  const value = useMemo(() => ({ ...parent, enter: false }), [parent]);
  return <MotionContext.Provider value={value}>{children}</MotionContext.Provider>;
}

/**
 * Outside a provider the policy is the strictest sensible default — `reduced`
 * following the OS — never `full`: a component used before the host is wired
 * must not animate more than the host would allow.
 */
const FALLBACK: MotionContextValue = {
  mode: "reduced",
  profile: "operational",
  preference: "reduced",
  osReduced: false,
  enter: true,
  recipe: (name) => clampRecipe(name, "reduced", "operational"),
};

export function useMotion(): MotionContextValue {
  return useContext(MotionContext) ?? FALLBACK;
}

/**
 * The nearest provider's resolved policy, or `null` outside any provider (no fallback). For adapters that must
 * keep a vendored component's own rule when no host is mounted — the chart kit (`charts-kit/use-kit-reduced-motion.ts`)
 * keeps the lab's OS-only rule there, and obeys the clamped mode inside a host.
 */
export function useMotionScope(): MotionContextValue | null {
  return useContext(MotionContext);
}
