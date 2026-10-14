import { describe, expect, it } from "vitest";
import {
  clampRecipe,
  MOTION_PROFILES,
  MOTION_PREFERENCES,
  MOTION_RECIPES,
  RECIPES,
  resolveMotionMode,
  staggerDelayMs,
} from "./policy";

describe("resolveMotionMode", () => {
  it("user 'off' and profile 'none' win over everything", () => {
    expect(resolveMotionMode({ profile: "expressive", preference: "off", osReduced: false })).toBe(
      "none",
    );
    expect(resolveMotionMode({ profile: "none", preference: "system", osReduced: false })).toBe(
      "none",
    );
  });
  it("OS reduced motion wins over a requested expressive profile", () => {
    expect(
      resolveMotionMode({ profile: "expressive", preference: "system", osReduced: true }),
    ).toBe("reduced");
  });
  it("user 'reduced' wins even when the OS allows motion", () => {
    expect(
      resolveMotionMode({ profile: "operational", preference: "reduced", osReduced: false }),
    ).toBe("reduced");
  });
  it("full only when surface, user and OS all allow it", () => {
    expect(
      resolveMotionMode({ profile: "operational", preference: "system", osReduced: false }),
    ).toBe("full");
  });
  it("is monotone: no input combination raises the mode above 'system + os allows'", () => {
    const rank = { none: 0, reduced: 1, full: 2 } as const;
    for (const profile of MOTION_PROFILES)
      for (const preference of MOTION_PREFERENCES)
        for (const osReduced of [true, false]) {
          const mode = resolveMotionMode({ profile, preference, osReduced });
          const ceiling = resolveMotionMode({
            profile: "expressive",
            preference: "system",
            osReduced,
          });
          expect(rank[mode]).toBeLessThanOrEqual(rank[ceiling]);
        }
  });
});

describe("clampRecipe", () => {
  it("an operational surface cannot run showcase; it degrades to section", () => {
    const r = clampRecipe("showcase", "full", "operational");
    expect(r.effective).toBe("section");
    expect(r.clampedBy).toBe("profile:operational");
    expect(r.durationMs).toBe(RECIPES.section.maxMs);
  });
  it("an expressive region may run showcase at full", () => {
    expect(clampRecipe("showcase", "full", "expressive")).toMatchObject({
      effective: "showcase",
      durationMs: 400,
      clampedBy: null,
    });
  });
  it("none mode makes every recipe instant", () => {
    for (const r of MOTION_RECIPES)
      expect(clampRecipe(r, "none", "expressive")).toMatchObject({
        effective: "instant",
        durationMs: 0,
        distancePx: 0,
      });
  });
  it("reduced mode removes spatial motion and keeps only a brief opacity change for reveal/section/panel", () => {
    for (const r of ["reveal", "section", "panel"] as const) {
      const e = clampRecipe(r, "reduced", "operational");
      expect(e.distancePx).toBe(0);
      expect(e.opacityOnly).toBe(true);
      expect(e.durationMs).toBeLessThanOrEqual(180);
    }
    expect(clampRecipe("feedback", "reduced", "operational").effective).toBe("instant");
    expect(clampRecipe("showcase", "reduced", "expressive").effective).toBe("instant");
  });
  it("emphasis is colour-only, so reduced mode keeps it (no motion involved)", () => {
    expect(clampRecipe("emphasis", "reduced", "operational")).toMatchObject({
      effective: "emphasis",
      distancePx: 0,
      opacityOnly: false,
    });
    expect(RECIPES.emphasis.animates).toEqual(["color"]);
  });
  it("an unknown recipe name from a generator is instant, never a guess", () => {
    expect(clampRecipe("bounce-in", "full", "expressive")).toMatchObject({
      effective: "instant",
      clampedBy: "unknown-recipe",
    });
    expect(clampRecipe("", "full", "expressive").effective).toBe("instant");
  });
  it("no recipe animates layout and none exceeds the §8.2 ceilings", () => {
    for (const spec of Object.values(RECIPES)) {
      expect(spec.animates.every((p) => ["opacity", "transform", "color"].includes(p))).toBe(true);
      expect(spec.maxMs).toBeLessThanOrEqual(400);
      if (spec.recipe !== "showcase") expect(spec.maxMs).toBeLessThanOrEqual(280);
    }
  });
});

describe("staggerDelayMs", () => {
  it("steps 24 ms, caps at 150 ms, and is zero outside full mode", () => {
    expect(staggerDelayMs(0, "full")).toBe(0);
    expect(staggerDelayMs(1, "full")).toBe(24);
    expect(staggerDelayMs(5, "full")).toBe(120);
    expect(staggerDelayMs(6, "full")).toBe(150);
    expect(staggerDelayMs(40, "full")).toBe(150);
    expect(staggerDelayMs(3, "reduced")).toBe(0);
    expect(staggerDelayMs(3, "none")).toBe(0);
  });
});
