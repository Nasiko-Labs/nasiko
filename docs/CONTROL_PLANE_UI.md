# Control Plane UI

## Approach

The control-plane UI is static HTML + vanilla JS Web Components — no framework, no client-side
router, no build step. Each page is a separate `.html` file that loads browser-native ES modules.
The whole UI is **embedded into the server binary** at compile time via `rust-embed`
(`server/src/main.rs`) and served same-origin by the server's fallback handler, so the CORS
allowlist can default to empty and there is no separate frontend deployment.

**Auth**: Cookie-based JWT. The server sets `Set-Cookie: access_token=<jwt>; HttpOnly; Path=/;
SameSite=Strict` on login. Every `/api/*` route is guarded by the `require_auth` middleware
(`server/src/auth/middleware.rs`), which validates the JWT from the `Authorization: Bearer` header
or the `access_token` cookie and enforces token revocation fail-closed. Because the cookie is
HttpOnly, the UI can never inspect auth state locally — it only learns it from server responses
(a `401` triggers a redirect to `/login.html`, see `common/services/api.js`).

## File Layout

```
ui/
├── common/                      # shared, page-independent code (served at /common/*)
│   ├── components/              # ~45 web components (one .js, optional sibling .css each)
│   ├── services/
│   │   ├── api.js               # apiFetch()/fetchApi() — the single /api/* funnel + 401 handling
│   │   ├── sse.js               # connectSSE() EventSource helper
│   │   └── auth-service.js      # current-user cache backed by GET /api/me
│   ├── utils/                   # icons.js, toast.js, async-button.js, markdown.js, ansi.js,
│   │                            # stream-utils.js, date-utils.js, keyboard-shortcuts.js, ...
│   ├── styles/                  # shared utility CSS (btn, badge, surface, layout, ...)
│   ├── fonts/                   # vendored TTFs — no CDN
│   ├── global.css               # design tokens
│   └── components.css           # @import aggregator for common/styles/*
└── web/                         # one .html per page (served at /)
    ├── common → ../common       # symlink so /common/* resolves during local preview
    ├── index.html               # Orchestrator (chat input → routing engine)
    ├── ... (page .html files, see inventory below)
    ├── navigation.js            # window.fetchNavigation + shared window.fetch* data functions
    └── *.preview.js             # per-page preview fixtures (screenshot/preview harness)
```

`common/components/` splits into two kinds:

- **Generic components** — `app-header` (sidebar/top-bar nav), `app-modal`, `app-toast`,
  `app-badge`, `app-button`, `app-tabs`, `app-stat-card`, `app-line-chart`, `app-skeleton`,
  `app-empty-state`, `base-layout`, `data-view` (paginated card/list), `smart-table` (paginated
  table), `voice-input` (chat input with recording/transcription), `autocomplete`, and friends.
- **Page components** — one `<name>-page.js` per page (`your-agents-page.js`,
  `orchestrator-page.js`, `builds-page.js`, ...) that owns that page's rendering, events, and API
  calls, with its CSS in a sibling `<name>-page.css`.

## Page Inventory

| Page | File | Notes |
|------|------|-------|
| Orchestrator | `index.html` | Chat input → `POST /api/orchestrator/a2a` (SSE streaming) |
| Agents | `agents.html` | The whole Agents module in one document — `?view=hub` (catalog browser, default), `?view=your-agents` (deployed containers: status, restart/stop/start), `?view=import` (deploy methods: upload, registry import, GitHub), `?view=builds` (build jobs with SSE live progress). See "Module pages" below |
| Chat | `chat.html?agent_id=X` | Direct chat with one agent via the A2A proxy |
| Observability | `sessions.html` | The whole Observability module in one document — `?view=history` (execution history: every query across agents, default), `?view=flows` (multi-agent flow list), `?view=resources` (live host/container CPU, memory and IO; admin-only endpoint). See "Module pages" below |
| Observability Session | `observability-session.html?session_id=X` | The single trace view: chat transcript │ span tree │ span detail (Info/Attributes). Optional `&trace_id=` preselects one turn's trace. Polls briefly on open, since agent spans batch in a few seconds after a reply |
| Session Trace | `session-trace.html` | **Redirect stub.** Resolves `?trace_id=` → session via the trace's `project_session_id` and forwards to `observability-session.html`; kept so old links survive |
| Flow Detail | `flow.html?id=X` | Summary cards + hop/step trace |
| Usage | `usage.html` | Token/cost stat cards + by-agent/by-model tables |
| Settings | `settings.html` | The whole Settings module in one document — `?view=settings` (runtime-editable config, default; its own General / Flow limits / Registry / Single sign-on sections are addressed by the same param, e.g. `?view=limits`), `?view=secrets` (encrypted secrets CRUD). See "Module pages" below |
| Agent Card | `agent-card.html?id=X` | Structured A2A AgentCard view |
| Login | `login.html` | Credential login (sets the auth cookie) |

Sidebar navigation is defined in `web/navigation.js` (`window.fetchNavigation`), rendered by
`app-header` (top bar below 1024px, left sidebar at ≥ 1024px).

Organization/administration pages (users, teams, departments, access control, agent runtime, SSO
group mappings) are available in the Nasiko enterprise edition; some of their page components live
in `common/components/` so both editions share a single component set.

## Serving & Embedding

`server/src/main.rs` embeds two asset trees:

```rust
#[derive(Embed)]
#[folder = "../ui/web/"]
struct OssAssets;

#[derive(Embed)]
#[folder = "../ui/common/"]
#[prefix = "common/"]
struct CommonAssets;
```

`build_app(state, static_handler)` mounts `static_handler` as the router fallback: anything that
doesn't match an API route is looked up in `OssAssets`, then `CommonAssets`. An empty path serves
`index.html`; misses serve `404.html`. Every asset response carries an ETag (embedded sha256) and
`Cache-Control: max-age=300, must-revalidate` — assets aren't content-hashed, so the short max-age
bounds post-deploy staleness to ~5 minutes without hard refreshes.

There is no `ServeDir`, no UI directory env var, and no separate static file server — the binary is
self-contained.

## How Pages Talk to the API

**Single funnel rule**: every `/api/*` request goes through `apiFetch()` (raw `Response`, for
streaming/status-branching callers) or `fetchApi()` (JSON in/out, throws on non-2xx) from
`common/services/api.js`, so "session missing or expired" is handled in exactly one place. Never
call `window.fetch('/api/...')` directly from a component. SSE endpoints go through `connectSSE()`
in `common/services/sse.js`.

Endpoints used by the UI (verified against the server routers in `server/src/lib.rs` and the
modules below):

| Endpoint | Purpose | Server module |
|----------|---------|---------------|
| `POST /api/orchestrator/a2a` | Chat entry — JSON-RPC `message/send` / `message/stream` (SSE) | `server/src/router/` |
| `GET /api/agents`, `GET /api/agents/{id}` | Agent catalog list/detail (also `/acl`, `/visibility`, `/{id}/secrets`) | `server/src/agents/`, `server/src/catalog.rs` |
| `POST /api/agents/{id}` | A2A proxy — direct chat with one agent (UUID) | `server/src/agent_proxy.rs` |
| `POST /api/containers/{name}/stop\|start\|restart\|scale` | Container lifecycle actions | `server/src/admin/routes.rs` (nested at `/containers`) |
| `GET /api/builds`, `/api/builds/{id}`, `/{id}/progress` (SSE), `/{id}/logs` | Build pipeline | `server/src/build/routes.rs` |
| `GET/POST /api/chat/sessions`, `/{id}/messages`, `/{id}/files` | Chat sessions, messages, file parts | `server/src/chat/routes.rs` |
| `GET /api/flows`, `/api/flows/{flow_id}` | Flow traces | `server/src/flows.rs` |
| `GET /api/usage/summary\|history\|by-agent\|by-model` | FinOps rollups | `server/src/usage/routes.rs` |
| `GET /api/observability/trace/{id}`, `/agent/{id}/stats`, `/agents/{id}/logs` | Trace detail, agent stats/logs | `server/src/observability/` |
| `GET/POST/DELETE /api/secrets` | Vault secrets CRUD | `server/src/secrets/` |
| `GET/PUT /api/settings` | Runtime settings (write is admin-gated) | `server/src/settings.rs` |
| `GET /api/me`, `POST /api/auth/logout` | Session identity / logout | `server/src/auth/login.rs` |
| `POST /api/transcribe` | Voice input transcription | `server/src/transcribe.rs` |
| `POST /api/import/upload`, `/api/import/registry` | Add-agent on-ramps | `server/src/agents/` |

**Pagination convention**: the backend takes `limit` + `offset`; components call data functions as
`(query, page, limit)`. Service functions convert internally:

```js
const offset = (page - 1) * limit;
const params = new URLSearchParams({ limit, offset });
```

## Page & Component Patterns

### Page structure

Each `.html` file is a minimal skeleton (~25–40 lines): token/style links, three module scripts,
and the page component element (optionally wrapping static skeleton markup for zero-CLS loading):

```html
<head>
  <link rel="stylesheet" href="/common/global.css" />
  <link rel="stylesheet" href="/common/components.css" />
  <script type="module" src="/navigation.js"></script>
  <script type="module" src="/common/components/app-header.js"></script>
  <script type="module" src="/common/components/your-agents-page.js"></script>
</head>
<body>
  <app-header></app-header>
  <your-agents-page><!-- skeleton markup --></your-agents-page>
</body>
```

Load `navigation.js` first (it defines the `window.fetch*` data functions), then `app-header`, then
the page component.

### Module pages

A module with a nested sidebar (Orchestrator, Agents, Observability, Settings, MCP gateway) is
**one** document holding every view in that sidebar. Clicking a sidebar row shows a different view
in place; the shell, the topbar, and the sidebar itself are never rebuilt, refetched, or
re-animated. Only moving between *modules* is a real navigation.

This is not client-side routing: nothing is fetched on a switch and no history entry is pushed. The
active view is still named in the URL — `?view=<key>`, read once on load and rewritten with
`replaceState` on each switch — so a view can be linked, shared, and reloaded. `app-tabs` uses the
same contract via its `query-param` attribute; both go through `common/utils/module-view.js`, which
is the only place the param is spelled.

`module-shell` owns the nav and the view swapping:

```html
<module-shell module="agents" default-view="hub">
  <app-module-nav module="agents"></app-module-nav>
  <!-- Default view: real markup, so it paints before any module loads. -->
  <agents-page data-view="hub" data-title="Agent hub"><!-- static shell --></agents-page>
  <!-- Other views: a <template> keeps the element un-upgraded, so it runs no
       code and issues no requests until first opened. -->
  <template data-view="builds" data-title="Builds"
            data-module="/common/components/builds-page.js">
    <builds-page></builds-page>
  </template>
</module-shell>
```

Rules when adding or merging a view:

- The `data-view` keys **must** match the `section` keys in that module's `MODULE_NAVS` entry
  (`navigation.js`, and `ee/ui/web/navigation.js` for pages EE also serves). That pairing is the
  whole nav contract; a mismatch shows a highlighted row with no content.
- View components must **not** render an `app-module-nav` of their own — the shell owns it, and a
  nav inside a view would be destroyed on every switch, which is the thing this pattern exists to
  prevent.
- Each view's sheet is `<link>`ed by the page, since a sheet imported by a lazily-loaded module
  arrives after the view is already on screen.
- Geometry: as the direct body child, `module-shell` is the white content card, and on desktop it
  carries the nav's left gutter. Views inside it therefore do **not** take that gutter — see the
  `module-shell` block in `common/styles/page-layout.css`.
- Anything linking to a view uses `?view=` (`/agents.html?view=builds`), including the rail and ⌘F
  entries in `navigation.js` and any server-side redirect.
- A view may own a second, finer level of nav rows — Settings does: `settings-page` holds General /
  Flow limits / Registry / Single sign-on as panels of the one `settings` view. Those `section` keys
  are deliberately *not* view keys, so `module-shell` ignores them (it drops any key it has no
  `data-view` for) and the view's own `module-nav-select` listener answers instead. Three
  consequences: the listener goes on the shell, not on the view, because the nav is the shell's
  child and the event no longer bubbles through the view; the view resolves its initial section from
  the same `?view=` param via `initialView()`, and must be the shell's `default-view` so an inner
  key falls back to it rather than to nothing; and when the row is clicked while a sibling view is
  up, the view calls `shell.show()` on itself and then re-writes the URL and the nav highlight with
  the finer key, which `show()` has just coarsened.

### Static shell

Most pages fill that skeleton slot with the `.pre-*` classes from `common/styles/not-defined.css`
— a parallel, page-agnostic vocabulary that mirrors the loaded geometry, because the component's
own sheet isn't loaded yet.

`agents.html` takes the other route: the page `<link>`s the component's
sheet, so the slot can hold the component's **real** markup and class names (title, description,
search, toolbar) plus the component's own skeletons for the API-fed regions. The component then
never wipes `innerHTML` — `connectedCallback` binds to the existing nodes and renders only into
its data containers, falling back to rendering the shell when a host doesn't supply it. One set of
class names, styled identically before and after upgrade.

### Component CSS

Page/component CSS lives in a sibling `.css` file, imported with a CSS module import attribute and
adopted document-wide:

```js
import styles from "./sessions-page.css" with { type: "css" };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];
```

Scope selectors to the component's tag name (or use `@scope`) since sheets are document-adopted.

A sheet imported this way only exists once its module does, so it cannot style anything the page
paints before then. Where a page authors the component's real markup in HTML (see "Static shell"
below), the page `<link>`s the sheet instead and the module drops the import — one sheet, one
owner, either way. `agents.html` is the page on that variant — and because it hosts a module's
worth of views, it `<link>`s each view's sheet (see "Module pages").

### data-view / smart-table contract

- `data-fn` attribute names a `window[fn]` function.
- Signature: `async (query, page, limit) => ({ data: [...], total: N })` — both components use the
  same argument order and read `response.data` / `response.total` (a bare array also works).
- The function is resolved from `window` lazily on refresh.
- `item-component` (data-view) names a custom element that receives each row via an
  `itemData` property setter.

### Item components (for data-view)

1. Define `set itemData(obj)` and render inside it — `data-view` assigns `itemData` before
   appending to the DOM, so `connectedCallback`-only rendering misses the data.
2. Ensure the item component's module is loaded **before** `data-view` renders: if the custom
   element isn't defined yet, the property lands on a plain `HTMLElement` and the setter never
   fires (upgrades do not re-run property setters).

```js
class MyItem extends HTMLElement {
  #data = null;
  set itemData(data) { this.#data = data; this.#render(); }
  get itemData() { return this.#data; }
  #render() { if (this.#data) this.innerHTML = `...`; }
}
customElements.define('my-item', MyItem);
```

### Conventions

- **Private fields**: use `#private` fields/methods, not `_underscore` prefixes.
- **Icons**: all icons come from `/common/utils/icons.js` — no inline SVG in HTML files.
- **Toasts**: `app-toast` uses `popover="manual"` for top-layer rendering above modals;
  fire via `showToast()` from `/common/utils/toast.js`.
- **Button loading**: wrap async click handlers with `withLoading()` from
  `/common/utils/async-button.js` (disables the button + spinner while pending).
- **Navigation**: use `<a>` tags, not click listeners + `window.location`.

## Design System

Tokens live in `ui/common/global.css`; shared utility classes in `ui/common/styles/*` are
aggregated by `ui/common/components.css`.

- **Palette**: yellow brand (`yellow-600 #BB8F06` accents in light, `yellow-400 #EEC239` in dark),
  Slate neutral ramp. Feedback colors: green/orange/red/blue 600-level fg on 100-level bg.
- **Fonts**: Chivo Mono (display h1/h2/logotype + code, weight 500), Inter (body/UI). Vendored
  TTFs in `ui/common/fonts/` — no CDN.
- **Tokens**: DS names (`--bg-*`, `--fg-*`, `--border-*`, `--s-*`, `--r-*`) are canonical; legacy
  `--color-*` / `--space-*` / `--radius-*` names are aliases onto them. Use DS names in new code.
- **Theme**: `light-dark()` follows the OS; `<html data-theme="light|dark">` pins it explicitly.
  Users pick Light/Dark/System from the avatar menu (`app-user-menu`); the choice persists in
  `localStorage["app-theme"]` and is applied on load by `common/utils/theme.js` (imported for its
  side effect by `app-header` and `login-page`).
- Text on brand/feedback action surfaces must use `var(--color-on-primary)` — never hardcoded
  `white`.
- Elevation is flat: 1px borders carry depth; shadows only on card hover (`--shadow-card-hover`)
  and menus (`--shadow-menu`).
- **Icons**: stroke 1.5, round caps, sizes 16/20/24/28 via `--icon-*`.

## Preview Fixtures

Each page has a `<pagename>.preview.js` next to its HTML file, consumed by a screenshot/preview
harness for developing pages without a running backend:

```js
export default {
  fetch: [
    ["GET /api/some/path", { ...response }],          // string key (query params auto-matched)
    ["POST /api/other", (req) => ({ ...dynamic })],   // function form
  ],
  window: {
    fetchMyData: async (query, page, limit) => ({ data: [...], total: N }),
  },
  scenarios: {
    "some-state": async (page) => { await page.click("..."); },
  },
};
```

Always use **string keys** for fetch fixtures (RegExp objects don't survive serialization in the
live-serve path). String keys match with or without query params: `"GET /api/foo"` matches both
`/api/foo` and `/api/foo?bar=1`.
