# Mock data and mock mode (harness only)

Ported from nasiko-ui-lab@1b99bdda `common/src/mocks/` — every lab mock and its tests. Every file starts with its
source path and its local edits. The Enterprise layer's mocks live, private, with the Enterprise edition (see
"Editions" below).

## What is mocked

One seed feeds everything (`seed.ts`, deterministic PRNG, dates anchored to "now" or `?anchor=`), with its own
seeds for coding harnesses (`seed-harness.ts`, ids `5eed0002-*`) and MCP servers (`seed-mcp.ts`). The handlers in
`handlers.ts` are grouped by `MOCKABLE` key; each group derives its answers from the seeds and mirrors the server's
quirks (envelopes, plain-text errors, sparse arrays, name forms), citing the Rust handler:

| Group                                                                      | Module                                   | Answers                                                                                              |
| -------------------------------------------------------------------------- | ---------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| `auth`                                                                     | handlers.ts                              | `/api/me`, login, logout, change-password                                                            |
| `agents`                                                                   | agents.ts                                | the agents directory, detail, lifecycle, versions, roll back, secrets, grants, stats, logs, users    |
| `dashboard`, `spend-timeseries`, `spend-calendar`, `top-traces`, `savings` | aggregate.ts                             | the finops reads (TokenOps), from the seed traces plus the admin's harness turns                     |
| `providers`                                                                | handlers.ts                              | the LLM-router provider catalog (custom providers from the router mock)                              |
| `flows`                                                                    | flows.ts                                 | `/api/flows*` (before observability and chat: it answers its own flows' traces)                      |
| `observability`                                                            | observability.ts, spanBuilder.ts         | sessions, traces, spans, agent logs and the log stream                                               |
| `harnesses`                                                                | harnessUsage.ts                          | the coding-agent usage endpoint (OSS rules), harness agents and chats, `/api/users/me`               |
| `chat`                                                                     | chat.ts (stream scenarios), chatStore.ts | chats, messages, the routed and direct streams, pending requests, `/api/hitl`                        |
| `router`                                                                   | router.ts, budgets.ts                    | LLM configs, routing, secrets, model registry, custom providers, by-agent usage, budgets             |
| `deploy`                                                                   | deploy.ts                                | uploads, builds and their stream, GitHub, registry import                                            |
| `settings`                                                                 | handlers.ts                              | `/api/settings`                                                                                      |
| `mcp`                                                                      | mcp.ts, seed-mcp.ts                      | connectors, toolkits, uploads, credentials, agent rules                                              |
| `workflows`                                                                | workflows.ts                             | `/api/maf/*` (NAS-697 shape; `workflows-classic` answers as `main`)                                  |
| `onboarding`                                                               | handlers.ts                              | `/api/me/onboarding`                                                                                 |
| `optimization`                                                             | optimization.ts, contextSavings.ts       | context strategy and budget, the proposed tiers, preview, settings history and context-savings reads |

Plus `/health` and the destination-only `slowHandlers`. Tests reach the live mock state through
`deployMockState()`, `routerMockState()`, `budgetMockState()`, `mcpMockState()`, `workflowsMockState()` and
`flowsMockState()`; an edition's mocks build on it through `mockCtx`.

Endpoints no released server has (the lab's "proposed" contracts, mocked so the pages can be built against them):
`GET /api/observability/finops/top-traces`; `GET /api/observability/finops/context-savings[/agents|/top-requests]`
(CX-V3/V3a/b/c); `/api/budgets*` (R-L10); `GET /api/observability/coding-agents/usage` (and its EE org rules);
`enabled` on `/api/me/context-strategy` (CX-5), `GET/PUT /api/settings/context-tiers` (CX-T1), `POST
/api/me/context-preview` (CX-6), `GET /api/me/settings-history` (CX-H) and the `nasiko.context.*` span attributes
(CX-V1). From unmerged server branches: `/api/me/onboarding` (`feat/user-onboarding-persona`), the NAS-697 workflow
fields, and EE's `/api/org-policy*` (`feat/l0-org-policy`). The `?mock=…-classic`/`…-absent` variants answer as
today's server.

## Running it

```sh
pnpm --filter @nasiko/web dev:mock
```

`VITE_NASIKO_API_MODE=mock` is a public build-time literal. The app's lazy mock boot starts the MSW
worker and gates child mounting until it answers. Edition handlers stack before the shared handlers.
There is no server-rendered identity: the same browser `/api/me` bootstrap reads the mock principal.
Persona selection and `configureMocks({ persona })` keep their existing semantics. If the worker fails,
the banner reports the failure; mock readiness never proves a backend endpoint exists.

Mock mode is admitted only by a development server or explicit `NASIKO_TEST_BUILD=1`, and is refused
in release builds. No seed, worker or mock dependency may enter a release artifact.

URL knobs, read on every request by the worker (so changing the URL is enough):

| Knob                                       | Effect                                                                                                           |
| ------------------------------------------ | ---------------------------------------------------------------------------------------------------------------- |
| `?anchor=YYYY-MM-DD`                       | Pins the seed and the page clock to 15:00 UTC that day (`anchor.ts`); `VITE_NASIKO_SEED_ANCHOR` is the env form  |
| `?as=<seed username>`                      | The harness viewer persona (`root`, `admin`, `maya`, `omar`, `lena`, `tom`, `sam`)                               |
| `?superuser=0`                             | `/api/me` reports the admin without `is_superuser`                                                               |
| `?mock=<variant>[,<variant>…]`             | Degraded and page states, `MOCK_VARIANTS` in handlers.ts (comma list: one degraded state plus any page variants) |
| `?mock=empty`                              | A fresh server: no sessions, no spend and no agents (the lab's empties Sessions only)                            |
| `?mock=server-down` / `logout-unavailable` | `/health` fails like a stopped server / logout answers the fail-closed 401                                       |
| `?mock=top-traces-absent`                  | The proposed `/finops/top-traces` is a bare 404, as on today's nasiko-server (destination addition)              |
| `?mock=slow`                               | Holds every finops read for 8 s (combinable, e.g. `?mock=slow,empty`; destination addition)                      |

The other variants are the lab's, by page: Sessions (`tempo-down`, `trace-503`, `trace-500`, `scan-fail`), Harnesses
(`usage-404`, `drill-404`, `prev-fail`, `usage-500`, `all-unpriced`, `no-activity`), Deploy (`deploy-*`,
`builds-absent`), MCP (`mcp-*`), Workflows (`workflows-*`), Onboarding (`onboarding-absent`, `onboarding-done`),
Optimization (`optimization-classic`, `optimization-no-reports`, `optimization-down`), Flows (`flows-empty`,
`flows-absent`), Chat (`CHAT_PAGE_VARIANTS`) and the LLM router (`ROUTER_PAGE_VARIANTS`, `router-*`).

A mock sign-out survives a reload (`localStorage` `nasiko:mock-signed-out`); the onboarding row does too
(`nasiko:mock-onboarding`: the browser starts a first-time user, tests a finished one). Clear them to start over.

## Editions

The lab stacked an edition's handlers in front of the core's through its `@edition/mocks` alias. Here this package
never imports an edition; the composition root supplies them:

- `handlers.ts` exports `EditionHandlers` (handlers by `MOCKABLE` group) and `handlersFor(groups, edition)`.
- Each app has `src/mock-boot.tsx` (`EditionMockBoot`), rendered by the browser root through
  `RootApplication`'s `mockBoot` prop. The OSS app's passes nothing; an edition that adds handlers passes
  `loadEditionHandlers`, a dynamic import of its own mocks behind its own `import.meta.env.VITE_NASIKO_API_MODE === "mock"`
  literal, so a release build drops the import with the gate. `startMocks({ editionHandlers })` puts them first, as the lab did.
- An edition's tests stack them on the msw/node server themselves:
  `beforeEach(() => server.use(...Object.values(editionHandlers).flat()))` after `setupPinnedSeed()`.
- The architecture check's `mocks-stay-dev-only` rule rejects any import of `@nasiko/mocks` or an edition's mocks from a
  production module; the mock boots are the only way in.

## Tests

`node_modules/.bin/vitest run --project web shared/mocks/src/<file>.test.ts` (one file at a time on the build
container). Files that call `setupPinnedSeed()` (`@nasiko/testing/pinned-seed`) get the fixed clock, the seeds and the
msw/node server; the few that do not (chat, onboarding, handlers) start `node.ts`'s server themselves. `sse.test.ts`
(the lab's `lib/sse.test.ts`) lives here because it drives `mockStream`; the decoder is
`@nasiko/data-access/product`. `@nasiko/testing/gated-sse` steps a stream frame by frame for page tests.

Not ported (staged in `.port/mocks-pending/`): `seed-script.test.ts`, which runs the lab's `scripts/seed-trace-usage.ts`
(the `seed:live` tooling is not part of this workspace), and `bundle-boundary.test.ts`, which builds the lab's Vite
bundles (the destination release gate is described below).

## msw 3 (the lab uses 2.15)

`oss/app` pins `msw@3.0.1`. API differences the port meets:

- `onUnhandledRequest` is `onUnhandledFrame` (`worker.start`, `server.listen`); the strategies are the same.
- `msw/browser` is browser-only. `mock-boot.tsx` reaches it through a gated React lazy import;
  node tests use `msw/node`.
- In Node (msw/node, so Vitest), a mocked `fetch` resolves only when the body yields its first chunk, and a body that
  errors mid-stream ends cleanly instead of failing the read. So the expired continuation's open stream starts with an
  SSE comment (no decoder event), and a `routed-cut` stream reads as one that stops before its terminal frame. A
  page test that needs a dropped connection (the lab's `ChatPage.routed` "cut" case, `gatedSse.fail()`) cannot get one
  through msw/node; it has to hand the client its Response without msw (a stubbed `fetch` for that one request).
- The handler shapes (`http.get`, `HttpResponse.json/text/error`, `handler.info.path`, a resolver returning
  `undefined` to fall through) are unchanged.

## The worker script

`public/mockServiceWorker.js` (in each app) is generated by `pnpm exec msw init <app>/public --save=false` (msw
3.0.1; its `INTEGRITY_CHECKSUM` must match the installed package, so regenerate it after an msw upgrade). It is listed
in the root `.prettierignore` because msw asks that it stay unmodified.

Release builds remove `mockServiceWorker.js` from the output. Rust also refuses a selected SPA containing
that worker and returns 404 for `/mockServiceWorker.js` and `/harness` test paths in the release handler.
Development/test CSP admits self-hosted workers through `script-src 'self'`; no production nonce is needed.

## Gating

- `shared/config/vite.config.ts` defines the literal `import.meta.env.VITE_NASIKO_API_MODE`.
  Shared and edition mock boots gate their dynamic imports on it. The release defaults to `live`.
- `pnpm build` produces `oss/app/dist` (and each private composition root's own); none contains a mock worker
  or harness route implementation. Edition markers and `nasiko-spa.json` identify the build.
- An explicit mock test artifact can be built with:
  `NASIKO_TEST_BUILD=1 NASIKO_HARNESS=1 NASIKO_TEST_DIST_DIR=dist-test-mock VITE_NASIKO_API_MODE=mock pnpm --filter @nasiko/web build`.
  It carries the `test-only` marker, cannot be selected by the Rust release embed path, and stays separate
  from the egress suites' live synthetic-control-plane artifacts.
- Use `tooling/test/spa-server.mjs` only for local verification. Production static serving belongs to Rust.
  Do not report synthetic data, mock personas or page tests as real backend acceptance.
