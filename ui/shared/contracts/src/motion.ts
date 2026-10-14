/**
 * Motion vocabulary shared with the generator (contract §8.2: "the model can
 * request an approved semantic recipe, not durations, easing curves, keyframes
 * or CSS"). The host (@nasiko/ui/motion) clamps a request by surface profile,
 * user preference and OS setting. Anything not in this list is rendered
 * instant — never guessed.
 *
 * This module is deliberately zod-free so client bundles that only need the
 * vocabulary do not carry the schema runtime; the schemas are in
 * `motion-schemas.ts` (exported from the package root).
 */
export const MOTION_RECIPES = [
  "instant",
  "feedback",
  "reveal",
  "panel",
  "section",
  "emphasis",
  "showcase",
] as const;
export type MotionRecipe = (typeof MOTION_RECIPES)[number];

export const MOTION_PROFILES = ["operational", "expressive", "none"] as const;
export type MotionProfile = (typeof MOTION_PROFILES)[number];

export const MOTION_PREFERENCES = ["system", "reduced", "off"] as const;
export type MotionPreference = (typeof MOTION_PREFERENCES)[number];
