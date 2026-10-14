/**
 * Motion host policy (contract §8.2). Pure: no React, no DOM, so every clamping
 * rule is unit-tested in isolation and the same table drives CSS and JS.
 *
 * Three independent inputs decide what actually moves:
 *  - profile    — what the *surface* is allowed to request (`operational` by
 *                 default; `expressive` only inside an admitted region; `none`).
 *  - preference — what the *user* chose (`system` follows the OS, or `reduced`/`off`).
 *  - osReduced  — the OS `prefers-reduced-motion` media query.
 * The result only ever moves downward: nothing a surface or a generator requests
 * can raise motion above what the user and the OS allow.
 */

import {
  MOTION_RECIPES,
  type MotionPreference,
  type MotionProfile,
  type MotionRecipe,
} from "@nasiko/contracts/motion";

export { MOTION_PROFILES, MOTION_PREFERENCES, MOTION_RECIPES } from "@nasiko/contracts/motion";
export type { MotionPreference, MotionProfile, MotionRecipe } from "@nasiko/contracts/motion";

/** What the runtime will do, after clamping. Mirrors the CSS `data-motion` values. */
export type MotionMode = "full" | "reduced" | "none";

export interface MotionInputs {
  readonly profile: MotionProfile;
  readonly preference: MotionPreference;
  readonly osReduced: boolean;
}

export function resolveMotionMode(i: MotionInputs): MotionMode {
  if (i.profile === "none" || i.preference === "off") return "none";
  if (i.preference === "reduced" || i.osReduced) return "reduced";
  return "full";
}

/** The token a recipe reads its duration from, and what it may animate. */
export interface RecipeSpec {
  readonly recipe: MotionRecipe;
  /** CSS custom property (without `var()`) holding the max duration. */
  readonly durationToken: `--motion-${string}`;
  /** Max duration in ms at `full`; the token may be lower, never higher. */
  readonly maxMs: number;
  /** Properties the recipe is allowed to touch. Never layout. */
  readonly animates: readonly ("opacity" | "transform" | "color")[];
  /** Travel at `full`, in px. 0 = no spatial motion. */
  readonly distancePx: 0 | 4 | 8 | 16;
}

export const RECIPES: Readonly<Record<MotionRecipe, RecipeSpec>> = {
  instant: {
    recipe: "instant",
    durationToken: "--motion-instant",
    maxMs: 0,
    animates: [],
    distancePx: 0,
  },
  feedback: {
    recipe: "feedback",
    durationToken: "--motion-feedback",
    maxMs: 120,
    animates: ["color", "opacity", "transform"],
    distancePx: 0,
  },
  reveal: {
    recipe: "reveal",
    durationToken: "--motion-micro",
    maxMs: 180,
    animates: ["opacity", "transform"],
    distancePx: 4,
  },
  panel: {
    recipe: "panel",
    durationToken: "--motion-panel",
    maxMs: 240,
    animates: ["opacity", "transform"],
    distancePx: 16,
  },
  section: {
    recipe: "section",
    durationToken: "--motion-section",
    maxMs: 280,
    animates: ["opacity", "transform"],
    distancePx: 8,
  },
  emphasis: {
    recipe: "emphasis",
    durationToken: "--motion-micro",
    maxMs: 180,
    animates: ["color"],
    distancePx: 0,
  },
  showcase: {
    recipe: "showcase",
    durationToken: "--motion-showcase",
    maxMs: 400,
    animates: ["opacity", "transform"],
    distancePx: 16,
  },
};

/** What a recipe becomes under a mode. */
export interface EffectiveRecipe {
  readonly requested: MotionRecipe;
  readonly effective: MotionRecipe;
  /** Duration in ms the JS runtime should use (CSS reads the token instead). */
  readonly durationMs: number;
  readonly distancePx: number;
  /** `true` when only opacity may change (reduced mode). */
  readonly opacityOnly: boolean;
  /** Why it was lowered, for diagnostics; `null` when honoured as requested. */
  readonly clampedBy:
    null | "mode:none" | "mode:reduced" | "profile:operational" | "unknown-recipe";
}

/**
 * Clamp a requested recipe. `showcase` requires the `expressive` profile; in an
 * operational surface it degrades to `section`. Reduced mode removes spatial
 * motion and keeps a brief opacity change for reveal/section/panel only (§8.2);
 * emphasis keeps its colour change (no motion involved); feedback becomes instant.
 */
export function clampRecipe(
  requested: string,
  mode: MotionMode,
  profile: MotionProfile,
): EffectiveRecipe {
  if (!isMotionRecipe(requested)) {
    return {
      requested: "instant",
      effective: "instant",
      durationMs: 0,
      distancePx: 0,
      opacityOnly: false,
      clampedBy: "unknown-recipe",
    };
  }
  let effective: MotionRecipe = requested;
  let clampedBy: EffectiveRecipe["clampedBy"] = null;
  if (requested === "showcase" && profile !== "expressive") {
    effective = "section";
    clampedBy = "profile:operational";
  }
  if (mode === "none") {
    return {
      requested,
      effective: "instant",
      durationMs: 0,
      distancePx: 0,
      opacityOnly: false,
      clampedBy: "mode:none",
    };
  }
  const spec = RECIPES[effective];
  if (mode === "reduced") {
    if (effective === "emphasis")
      return {
        requested,
        effective,
        durationMs: spec.maxMs,
        distancePx: 0,
        opacityOnly: false,
        clampedBy: clampedBy ?? "mode:reduced",
      };
    if (effective === "reveal" || effective === "section" || effective === "panel") {
      return {
        requested,
        effective,
        durationMs: RECIPES.reveal.maxMs,
        distancePx: 0,
        opacityOnly: true,
        clampedBy: clampedBy ?? "mode:reduced",
      };
    }
    return {
      requested,
      effective: "instant",
      durationMs: 0,
      distancePx: 0,
      opacityOnly: false,
      clampedBy: clampedBy ?? "mode:reduced",
    };
  }
  return {
    requested,
    effective,
    durationMs: spec.maxMs,
    distancePx: spec.distancePx,
    opacityOnly: false,
    clampedBy,
  };
}

export function isMotionRecipe(x: string): x is MotionRecipe {
  return (MOTION_RECIPES as readonly string[]).includes(x);
}

/** Stagger policy: ≤ 6 children, 24 ms each, cumulative delay capped at 150 ms. */
export const STAGGER = { stepMs: 24, maxChildren: 6, capMs: 150 } as const;

export function staggerDelayMs(index: number, mode: MotionMode): number {
  if (mode !== "full" || index <= 0) return 0;
  if (index >= STAGGER.maxChildren) return STAGGER.capMs;
  return Math.min(index * STAGGER.stepMs, STAGGER.capMs);
}

/** Easings: one ease-out, one ease-in-out. No overshoot for data/controls. */
export const EASING = {
  out: [0.2, 0, 0, 1] as const,
  inOut: [0.4, 0, 0.2, 1] as const,
};
