// @vitest-environment node
/**
 * Server rendering of the theme owner (ADR-0006 amendment 2026-10-08). The theme store is module-level, so on
 * the server one instance serves every request: the server must never read or write it per request, and its
 * HTML must not depend on any viewer's stored choice (storage is client-only; the pre-paint script sets `.dark`
 * and `data-theme` before first paint). Rendered here with the real server renderer, in Node, with a recording
 * global `localStorage` (Node ships one, process-wide) holding a different choice for each "request".
 */
import { renderToString } from "react-dom/server";
import { afterAll, describe, expect, it, vi } from "vitest";
import { THEME_PREPAINT_SCRIPT } from "./prepaint";
import { ACCENT_KEY, THEME_KEY, useThemePrefs } from "./theme-store";
import { UiProvider } from "./ui-provider";

// Installed before the modules above are evaluated (vi.hoisted runs first), so a storage read at import time —
// the store's own creation — is recorded too.
const rec = vi.hoisted(() => {
  const accesses: string[] = [];
  const state: { stored: Record<string, string> } = { stored: {} };
  const storage: Storage = {
    get length() {
      return Object.keys(state.stored).length;
    },
    clear: () => {
      accesses.push("clear");
    },
    getItem: (k) => {
      accesses.push(`get:${k}`);
      return state.stored[k] ?? null;
    },
    key: () => null,
    removeItem: (k) => {
      accesses.push(`remove:${k}`);
    },
    setItem: (k) => {
      accesses.push(`set:${k}`);
    },
  };
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: storage });
  return { accesses, state, original };
});

afterAll(() => {
  if (rec.original) Object.defineProperty(globalThis, "localStorage", rec.original);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

/** What a product component would render from the store (the shell's theme menu reads it the same way). */
function Probe() {
  const prefs = useThemePrefs();
  return <output data-theme-pref={prefs.theme} data-accent-pref={prefs.accent} />;
}

function page(nonce: string) {
  return renderToString(
    <html lang="en" suppressHydrationWarning>
      <head>
        <script nonce={nonce} dangerouslySetInnerHTML={{ __html: THEME_PREPAINT_SCRIPT }} />
      </head>
      <body>
        <UiProvider nonce={nonce}>
          <Probe />
        </UiProvider>
      </body>
    </html>,
  );
}

describe("theme owner on the server", () => {
  it("runs in a server environment (no window)", () => {
    expect(typeof window).toBe("undefined");
  });

  it("renders identical, theme-neutral HTML whatever each request's viewer has stored", async () => {
    const render = async (theme: string, accent: string) => {
      rec.state.stored = { [THEME_KEY]: theme, [ACCENT_KEY]: accent };
      await Promise.resolve();
      return page("n0nce");
    };
    // Two "requests" in flight together, each with a different stored choice.
    const [a, b] = await Promise.all([render("dark", "plum"), render("light", "teal")]);
    expect(a).toBe(b);
    // The store's server snapshot is the defaults, not anyone's choice.
    expect(a).toContain('data-theme-pref="system"');
    expect(a).toContain('data-accent-pref="carbon"');
    // Nothing on the server-rendered elements selects a mode or a colour theme: that is the pre-paint script's job.
    const markup = a.replace(/<script[\s\S]*?<\/script>/g, "");
    expect(markup).not.toMatch(/class="[^"]*\bdark\b/);
    expect(markup).not.toMatch(/\sdata-theme="/);
    // The pre-paint script ships under the request's nonce.
    expect(a).toContain('<script nonce="n0nce">');
  });

  it("never reads or writes storage (at import or per request), and rendering does not mutate the store", () => {
    rec.state.stored = { [THEME_KEY]: "dark", [ACCENT_KEY]: "indigo" };
    const first = renderToString(
      <UiProvider>
        <Probe />
      </UiProvider>,
    );
    const second = renderToString(
      <UiProvider>
        <Probe />
      </UiProvider>,
    );
    expect(rec.accesses).toEqual([]);
    // still the defaults after rendering: nothing a render does on the server changes the shared store
    expect(second).toBe(first);
    expect(second).toContain('data-theme-pref="system" data-accent-pref="carbon"');
  });
});
