# AGENTS.md — `oss/ui`

`oss/ui` is a zero-build, vanilla-JS web-component frontend: no bundler, no
framework, no `node_modules` at runtime. Every page is native ES modules +
CSS, embedded directly into the server binary and served same-origin.

It has two parts:

- **`common/`** — the single source of shared design-system assets: web
  components, CSS, fonts, icons, and small JS utilities/services. Nothing in
  here is specific to any one site.
- **`web/`** — one website's pages (`*.html`), each importing components
  from `common/` and defining its own page-level logic/styles.

`common/` is meant to be reused: any website root elsewhere in the repo can
pull it in by symlinking a `common` entry to this directory (see
`web/common` for the in-repo example: `ln -s ../common web/common`). A page
then references `/common/global.css`, `/common/components/*.js`, etc. and
gets the same design system, tokens, and components as every other site
doing the same thing. When adding a new website, prefer this symlink over
copying files — `common/` has exactly one copy in the repo.

## Layout

| Path | Contents |
| :--- | :--- |
| `common/global.css` | Design tokens (`--color-*`, `--space-*`, `--radius-*`, `--font-*`, …) and element defaults, in `@layer base` |
| `common/components.css` | Aggregator that `@import`s every file in `common/styles/` — link this once per page |
| `common/styles/*.css` | Utility layers: buttons, badges, layout primitives, prose/markdown, segmented controls, surfaces, text, the page host geometry (`page-layout.css`), and the `:not(:defined)` upgrade-contract rules |
| `common/components/*.js` (+ sibling `.css`) | Web components: reusable primitives (`app-button`, `app-modal`, `app-badge`, layout elements, …) and page-level components |
| `common/services/api.js` | Single funnel for `/api/*` calls (`apiFetch`/`fetchApi`) — handles session-expired redirects in one place |
| `common/services/sse.js` | `connectSSE()` helper wrapping `EventSource` |
| `common/utils/*.js` | `toast.js` (ephemeral feedback), `theme.js` (light/dark/system, persisted), `icons.js` (central SVG icon library), plus markdown, date, keyboard-shortcut, and async-button helpers |
| `common/vendor/*.esm.js` | Vendored single-file ESM builds of third-party libraries (see `common/vendor/README.md`) — committed directly since there's no package manager at runtime; never hand-edit, replace wholesale on upgrade |
| `common/fonts/`, `common/mark-nasiko.svg` | Vendored webfonts and the mark referenced by `global.css` / favicons |
| `web/*.html` | One page per file, each a real URL — no client-side router |
| `web/<pagename>.preview.js`, `web/.preview/` | Preview-only fixtures (see below) — never referenced by production pages |

## Component conventions

- **Light DOM only** — no `attachShadow()`. Style isolation comes from CSS
  `@scope`, not Shadow DOM.
- **`@scope` per component** — every component's rules are wrapped in
  `@scope (element-name) { ... }` and styled from `:scope`, so specificity
  stays local without `!important`.
- **Two ways to attach CSS**, depending on the component's weight:
  - Small shared primitives build a `CSSStyleSheet`, `replaceSync()` an
    inline `@scope` template literal, and push it onto
    `document.adoptedStyleSheets` — see `common/components/app-button.js`.
  - Page-level/feature components keep their CSS in a sibling `.css` file
    and pull it in as a CSS Module Script:
    `import styles from "./thing.css" with { type: "css" }`, then the same
    `document.adoptedStyleSheets` push.
- **Design tokens only** — colors, spacing, radii, and type come from the
  `--*` custom properties in `global.css`. Don't hardcode a hex value or
  a raw `px` size in component CSS.
- **Upgrade contract** — any component with layout footprint gets a
  `<element>:not(:defined) { ... }` rule in `common/styles/not-defined.css`
  that reserves the same geometry the real render will use, so there's no
  layout shift when the custom element upgrades. It must live in that file,
  not in the component's own CSS: a component's sheet is adopted by its
  module, so it does not exist at first paint.
- **Page host geometry lives in `common/styles/page-layout.css`** — a page
  element's own box (display, padding, the desktop card padding, the
  module-nav gutter) is written there once, keyed on the element and with no
  `:not(:defined)` filter, so the same rule serves both the pre-upgrade paint
  and the loaded page. A page's `@scope`d sheet must not re-declare those
  properties, or it wins on proximity and the two copies drift.
- **Mandatory JS shape** — `#private` fields for internal state; a
  connect guard (`#initialized` flag checked in `connectedCallback`) so
  re-attaching an element doesn't double-render; clean up timers/listeners
  in `disconnectedCallback` when a component adds any outside its own DOM.
- **Compose, don't hand-roll** — reach for `app-button`, `app-modal`,
  `app-badge`, `showToast()`, and `icons.js` instead of a raw `<button>`,
  a hand-built dialog, a status `<div>`, or inline SVG.
- **No client-side routing** — pages are real `.html` files linked with
  `<a href>`. Page-specific `window.*` data-fetching functions live in a
  plain script (see `web/navigation.js`) loaded before the components that
  call them.

## Previewing pages

The `ui` CLI (`ui shot` / `ui serve`) renders a page in a headless browser —
or serves it with live reload — without running the backend. It intercepts
`fetch()`/`EventSource` and replaces them with fixture data, so pages that
call `/api/*` still render fully offline.

```bash
# Live-reload dev server while editing (open the printed URL in your browser)
ui serve oss/ui/web --port 7777

# Overlay preview — extra roots are fallbacks (first dir wins), mirroring
# how a downstream distribution embeds its page overrides over these pages
ui serve <overlay-dir> oss/ui/web --port 7777

# Or proxy real API calls to a running backend instead of using fixtures
ui serve oss/ui/web --port 7777 --proxy http://localhost:8080

# Headless screenshot — no server, reads files straight from disk
ui shot oss/ui/web/<pagename>.html --out /tmp/<pagename>.png --full-page

# Responsive + theme matrix in one go
ui shot oss/ui/web/<pagename>.html --out /tmp/<pagename>.png \
  --devices mobile,desktop --themes light,dark --full-page
```

Use `--wait ".some-selector"` to wait for an async element before
capturing, rather than a fixed `--delay`.

### Fixture layers

Fixtures are preview-only mocks, injected by the `ui` tool at page load.
**Production HTML/components must never import or reference them.** Three
layers merge in order, later layers winning on key collision:

| Priority | File | Scope |
| :--- | :--- | :--- |
| 1 (lowest) | `common/.preview/fixtures.js` | Endpoints used by shared `common/components/*` widgets |
| 2 | `<site>/.preview/fixtures.js` (e.g. `web/.preview/fixtures.js`) | Endpoints shared across a site's pages (nav, current-user, etc.) |
| 3 (highest) | `<site>/<pagename>.preview.js`, next to the HTML file | Overrides for a single page |

```js
// web/<pagename>.preview.js
export default {
  fetch: [
    ["GET /api/things", [{ id: 1, name: "Example" }]],
    [{ method: "GET", path: /^\/api\/things\/\d+$/ }, { id: 1, name: "Example" }],
  ],
  window: {
    fetchThings: async () => [{ id: 1, name: "Example" }],
  },
  // Optional: scripted interactions for extra screenshot states.
  scenarios: {
    "expanded": async (page) => {
      await page.click("#expand-button");
      await page.waitForSelector(".expanded-content");
    },
  },
};
```

`fetch`/`window` values can be plain data or real functions (received the
`Request`, or called with no args); `sse` entries mock `new EventSource(url)`
with an ordered list of `{ delay?, data }` events. Run a scenario with
`ui shot ... --scenario expanded`, or `--scenario all` to capture the default
state plus every scenario the page defines in one invocation.

Add or update a fixture whenever a page/component starts calling a new
backend endpoint — without one, the call falls through to the real network
(which fails in file mode) and the page renders empty.
