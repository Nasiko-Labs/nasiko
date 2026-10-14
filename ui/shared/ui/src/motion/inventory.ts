import type { MotionRecipe } from "./policy";

/**
 * Motion inventory (contract §8.6): every admitted recipe with its trigger,
 * region, token, interruption behaviour, accessibility fallback and cost. This
 * is data so the Storybook matrix, the docs and the test suite read one source.
 */
export interface MotionInventoryEntry {
  readonly recipe: MotionRecipe;
  readonly adapter: string;
  readonly trigger: string;
  readonly region: string;
  readonly token: string;
  readonly interruption: string;
  readonly reducedFallback: string;
  readonly noneFallback: string;
  readonly cost: string;
  readonly evidence: string;
}

export const MOTION_INVENTORY: readonly MotionInventoryEntry[] = [
  {
    recipe: "panel",
    adapter: "workspace router <Reveal>",
    trigger: "incoming signed-in route commits; not a filter or search-param update",
    region: "page content only; sidebar, session and data providers remain mounted",
    token: "panel ≤240 ms / 16 px horizontal; nested reveal ≤180 ms / 4 px horizontal",
    interruption: "incoming only; no exit wait or duplicated outgoing screen; unmount cancels",
    reducedFallback: "opacity only",
    noneFallback: "render in place",
    cost: "one opacity/transform wrapper; no row stagger or new motion runtime",
    evidence: "router.navigation.test.tsx; policy.test.ts; local EE navigation smoke",
  },
  {
    recipe: "feedback",
    adapter: "CSS (`transition-colors duration-feedback`) on primitives",
    trigger: "hover / press / selection",
    region: "the control itself",
    token: "--motion-feedback (≤120 ms)",
    interruption: "CSS transitions retarget; no state waits on them",
    reducedFallback: "instant (token = 0)",
    noneFallback: "instant",
    cost: "composited colour/opacity; no layout",
    evidence: "test:motion › feedback",
  },
  {
    recipe: "reveal",
    adapter: "<Reveal> / <StaggerGroup>",
    trigger: "content that just became available (subtree valid, card arrived)",
    region: "the arriving element only",
    token: "--motion-micro (≤180 ms), 4 px travel, stagger 24 ms × ≤6, cap 150 ms",
    interruption: "mount-only; unmount cancels; restored surfaces never replay",
    reducedFallback: "opacity only, ≤180 ms",
    noneFallback: "render in place",
    cost: "opacity + transform on the element; measured in Phase 6",
    evidence: "test:motion › reveal, restored",
  },
  {
    recipe: "panel",
    adapter:
      '<Presence recipe="panel">, Dialog/Sheet/Popover primitives (tw-animate, duration-panel)',
    trigger: "open / close of drawer, dialog, popover, expandable region",
    region: "the panel; scrim is opacity only",
    token: "--motion-panel (≤240 ms), 16 px travel",
    interruption:
      "exit interruptible; focus restore and dismissal run immediately; exiting content is inert",
    reducedFallback: "opacity only",
    noneFallback: "immediate mount/unmount",
    cost: "opacity + transform; no height animation",
    evidence: "test:motion › panel, rapid-toggle",
  },
  {
    recipe: "section",
    adapter: '<Reveal recipe="section">',
    trigger: "nested route child region replaces its content",
    region: "the changed boundary only; parent layout untouched",
    token: "--motion-section (≤280 ms), 8 px travel",
    interruption: "navigation and data start immediately; superseded navigation cancels",
    reducedFallback: "opacity only",
    noneFallback: "render in place",
    cost: "one element; no shared-element duplication",
    evidence: "test:motion › nested-nav",
  },
  {
    recipe: "emphasis",
    adapter: "<Emphasis changeKey>",
    trigger: "a real value change (KPI refetch, new timeline event)",
    region: "the value span",
    token: "--motion-micro, colour only, ~4× duration then off",
    interruption: "re-triggers reset the timer; value is always current",
    reducedFallback: "kept (no motion involved)",
    noneFallback: "none",
    cost: "background-color transition on a span",
    evidence: "test:motion › emphasis (no count-up)",
  },
  {
    recipe: "showcase",
    adapter: "extensions (Magic UI / Cult UI) inside <ExpressiveRegion>",
    trigger: "admitted presentation contexts only",
    region: "the expressive region; never operational data",
    token: "--motion-showcase (≤400 ms); finite effect lifetime",
    interruption: "effects pause when off-screen; static fallback rendered first",
    reducedFallback: "instant / static",
    noneFallback: "static",
    cost: "separate lazy bundle (an Enterprise extension); measured in Phase 6",
    evidence: "test:motion › showcase-clamp",
  },
  {
    recipe: "instant",
    adapter: "default for every state the policy does not list",
    trigger:
      "errors, permission changes, scope resets, data correctness updates, high-frequency data",
    region: "—",
    token: "--motion-instant (0 ms)",
    interruption: "n/a",
    reducedFallback: "instant",
    noneFallback: "instant",
    cost: "none",
    evidence: "policy.test.ts",
  },
];
