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
| Agents | `agents.html` | Agent catalog browser |
| Your Agents | `your-agents.html` | Deployed containers: status, restart/stop/start actions |
| Add Agent | `add-agent.html` | Deploy methods: upload, registry import, GitHub |
| Chat | `chat.html?agent_id=X` | Direct chat with one agent via the A2A proxy |
| Sessions | `sessions.html` | Chat session history (sidebar + messages) |
| Session Trace | `session-trace.html` | Span tree for a trace (`/api/observability/trace/{id}`) |
| Flows | `flows.html` | Multi-agent flow list |
| Flow Detail | `flow.html?id=X` | Summary cards + hop/step trace |
| Builds | `builds.html` | Server-side build jobs with SSE live progress |
| Usage | `usage.html` | Token/cost stat cards + by-agent/by-model tables |
| Secrets | `secrets.html` | Encrypted secrets CRUD |
| Settings | `settings.html` | Runtime-editable config |
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

### Component CSS

Page/component CSS lives in a sibling `.css` file, imported with a CSS module import attribute and
adopted document-wide:

```js
import styles from "./your-agents-page.css" with { type: "css" };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];
```

Scope selectors to the component's tag name (or use `@scope`) since sheets are document-adopted.

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
