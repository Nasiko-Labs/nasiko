/* eslint-disable @typescript-eslint/no-non-null-assertion -- regex captures on a fixed stylesheet, as in the lab */
/**
 * CSS rules the theme depends on, ported from the lab's `common/src/app/shell/css.test.ts`
 * (nasiko-ui-lab 1b99bdda) and pointed at this package's tokens.css. The lab's mock-variant and
 * TokenOps checks have no counterpart here. Runs in the `unit` project (node).
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { ACCENTS } from "../theme/theme-store";

const SRC = fileURLToPath(new URL("../", import.meta.url));
const css = readFileSync(join(SRC, "styles/tokens.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** `selector { body }` blocks at the top level of tokens.css. */
function blocks(): { selector: string; body: string }[] {
  return [...css.matchAll(/(^|\n)([^{}\n][^{}]*?)\s*\{([^{}]*)\}/g)].map((m) => ({
    selector: m[2]!.trim().split("\n").pop()!.trim(),
    body: m[3]!,
  }));
}

describe("tokens.css tokens", () => {
  it("defines --primary-text once, as the primary itself", () => {
    const root = blocks().find((b) => b.selector === ":root")!;
    expect(root.body).toMatch(/--primary-text:\s*var\(--primary\);/);
    expect(
      blocks()
        .filter((b) => /--primary-text:/.test(b.body))
        .map((b) => b.selector),
    ).toEqual([":root"]);
  });

  it("maps every sidebar token onto an existing token", () => {
    const root = blocks().find((b) => b.selector === ":root")!;
    // The sidebar's "accent" is its hover fill, so it maps to --muted (the active row uses --accent).
    const map: Record<string, string> = {
      "sidebar-foreground": "foreground",
      "sidebar-primary": "primary",
      "sidebar-primary-foreground": "primary-foreground",
      "sidebar-accent": "muted",
      "sidebar-accent-foreground": "foreground",
      "sidebar-border": "border",
      "sidebar-ring": "ring",
    };
    for (const [t, to] of Object.entries(map)) {
      expect(root.body, t).toMatch(new RegExp(`--${t}:\\s*var\\(--${to}\\);`));
    }
    expect(css).not.toMatch(/--sidebar[a-z-]*:\s*hsl/);
  });

  it("has no retired accent presets, and status colours stay per mode, outside the theme blocks", () => {
    expect(css).not.toMatch(/data-accent/);
    for (const b of blocks().filter((x) => x.selector.includes("data-theme"))) {
      expect(b.body, b.selector).not.toMatch(/--(success|warning|destructive|info|logo):/);
    }
  });

  it("keeps the lab's radius base and the kit's chart variables on the app's tokens", () => {
    const root = blocks().find((b) => b.selector === ":root")!;
    expect(root.body).toMatch(/--radius:\s*0\.5rem;/);
    expect(css).toMatch(/--radius-lg:\s*var\(--radius\);/);
    expect(css).toMatch(/--radius-md:\s*calc\(var\(--radius\) - 2px\);/);
    expect(css).toMatch(/--radius-sm:\s*calc\(var\(--radius\) - 4px\);/);
    expect(css).toMatch(/--chart-grid:\s*var\(--border\);/);
    expect(css).toMatch(/--chart-line-primary:\s*var\(--chart-1-edge\);/);
  });

  it("reads the app's font variables, with the lab's families as the fallback", () => {
    expect(css).toMatch(/--font-sans:\s*var\(--font-hanken, "Hanken Grotesk Variable"\)/);
    expect(css).toMatch(/--font-mono:\s*var\(--font-jetbrains, "JetBrains Mono Variable"\)/);
    expect(css).not.toMatch(/@font-face/);
  });
});

describe("accent text class (design D3)", () => {
  it("colours accent text with text-primary-text, never text-primary, in any stylesheet", () => {
    const offenders: string[] = [];
    const walk = (dir: string) => {
      for (const f of readdirSync(dir)) {
        const p = join(dir, f);
        if (statSync(p).isDirectory()) walk(p);
        else if (f.endsWith(".css")) {
          readFileSync(p, "utf8")
            .split("\n")
            .forEach((line, i) => {
              if (/text-primary(?![-\w])/.test(line))
                offenders.push(`${p.slice(SRC.length)}:${i + 1}`);
            });
        }
      }
    };
    walk(SRC);
    expect(offenders).toEqual([]);
  });
});

describe("theme swatches", () => {
  it("show each theme's light --primary exactly", () => {
    const light = (sel: string) =>
      blocks()
        .find((b) => b.selector === sel)!
        .body.match(/--primary:\s*([^;]+);/)![1]!
        .trim();
    for (const a of ACCENTS) {
      const expected = light(`:root[data-theme="${a.id}"]`);
      expect(a.swatch.toLowerCase(), a.id).toBe(expected.toLowerCase());
    }
  });
});
