# UI companion: Request classification tab (outside the hackathon scope)

The hackathon submission is the core change in `llm-router/` (see PR-core.md). This
companion adds a small operator surface to the existing Router page and a thin management
API so the classifier can be seen and tried without a terminal. It depends on the core
change; the core does not depend on it, and neither the evaluator nor routing needs it.

## What it is

- **Server** (`server/src/llm_router/classifier.rs`):
  - `GET /api/llm-router/classifier` — configured vs effective backend (`regex`, `jev`,
    `laya`), init error, model/version, endpoint host (Jev) or model directory (Laya), timeout,
    confidence floor, seed flag, counters. Any authenticated user. Never carries a key.
  - `POST /api/llm-router/classifier/preview` — `{query, context?, backend: configured|regex}`
    → the requested backend's answer plus the regex baseline on the same input. Superuser
    only (a hosted preview is a paid call; the server has no finer operator role), rate
    limited per user (20/min), bounded input (8 000 / 4 000 chars). Side-effect free: it
    calls the router's own `ClassifierService` instance and nothing else — no decision
    cache, no learned cells, no config, no agent state. The browser never reaches the
    hosted backend; endpoint and key are deployment config and cannot be supplied by a
    caller.
  - Both are in OpenAPI (`openapi.rs`); `ui/common/src/lib/api/schema.gen.ts` was
    regenerated with `scripts/gen-api.ts` and only the classifier hunks were kept (see
    "Pre-existing drift" below).
  - `LlmRouterCtx` is now built before the `/api` router so the preview shares the exact
    classifier instance routing uses (one hosted client, one set of counters).
- **UI** (`ui/common/src/features/router/`): a fourth Router tab, **Classification**
  (`?tab=classification`), with:
  1. a status card: configured backend badge (Regex default / Jev hosted / Laya local),
     "Answers with", model version, endpoint host or model files, a "Local model loaded"
     badge for a working Laya, timeout, floor, counters since start; when the configured
     backend cannot start, the init error plus the exact setup path (env vars for Jev, the
     setup script for Laya); and a line naming the backends not configured here, which are
     therefore shown as not run rather than silently substituted;
  2. a form: labelled Query and optional Context textareas, a "Preview with" picker
     (configured backend / regex baseline, shown only when a hosted backend is
     configured) and an explicit **Test classification** button — nothing runs on
     keystrokes;
  3. results: one card per backend with readable request type, difficulty out of 5,
     confidence explained per backend (model = probability of the type; regex = fixed
     placeholder, not calibrated), measured decision latency including fallback, cost
     from usage only (Jev: tokens shown, "price not configured, cost unavailable"; Laya:
     tokens shown, "local inference, no API fee (CPU time only)"), the model version that
     answered, fallback reason or abstention, and a truncation note (Laya's says the
     request did not fit the 512-token window);
  4. states: loading, invalid input (local, nothing sent), unavailable/unconfigured
     backend (honest regex fallback card, never a Jev success), non-superuser (status
     visible, form disabled with the reason), timeout (504) and rate limit (429) mapped
     with Retry (re-sends the same input), stale-result protection when previews overlap,
     a polite announcement of the verdict.
  - Classification only: no model or tier is shown, because showing one would require
    the routing policy and the preview must not touch routing state.
  - Mock mode: `?mock=router-classifier-regex`, `router-classifier-unconfigured`,
    `router-classifier-fail`, `router-classifier-laya`, `router-classifier-laya-missing`;
    the shell's mock banner stays visible.
  - Only one model backend is loaded per deployment, so "compare available backends" means
    the configured model next to the regex baseline on the same input; the other model is
    labelled not configured / not run. Nothing is substituted when the selected backend is
    unavailable: its card shows the regex fallback with the init reason.

## Not done, on purpose

- No enable/save switch. Backend selection is deployment configuration; a settings-store
  toggle would need a new secrets reference, server validation and rollback for a
  one-line env var. The tab shows the effective config and the setup path instead.
- No transcript storage, no analytics dashboard, no price configuration.

## Verification

- Server: `cargo fmt --check`, `cargo clippy -p nasiko-server --all-targets` (0 warnings),
  `cargo test -p nasiko-server --lib` (all passing; 8 tests over loopback HTTP: status
  without secrets, a Laya deployment with a missing bundle, 403 for non-superuser, 400/422
  validation, result+baseline via the shared service, honest init fallback, regex selector,
  per-user 429).
- UI: `npm run typecheck`, `eslint --max-warnings 0`, `vitest` (router feature and full
  suite), `npm run build`; `ClassificationSection.test.tsx` (16 tests incl. axe, Laya
  loaded and Laya missing);
  Playwright `demo.spec.ts` gained a classification case with axe.
- Browser (mock mode): desktop and 375 px, dark and light, keyboard-only submit, no
  console errors, no horizontal overflow; no motion added.

## Pre-existing drift found while regenerating the client

`scripts/gen-api.ts` against the OSS server also emits `compress_enabled` /
`minimal_code_enabled` on the agent schemas and reworded EE doc comments, which the
committed `schema.gen.ts` lacks and which break `mocks/agents.ts` and
`mocks/observability.ts` type-checking. Those hunks were left out of this change; they
belong to a separate client-sync PR. `prettier --check` also flags two untouched files
(`common/surface/fixtures.js`, `common/surface/query-manager.js`).
