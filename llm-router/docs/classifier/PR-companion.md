# [classifier][ui] Request classification tab and preview API (companion to the core PR)

Builds on `[classifier] pluggable request classifier…`. **Outside the hackathon submission
scope** (touches `server/` and `ui/`); the core PR stands alone.

## What it adds

- `GET /api/llm-router/classifier` (status, no secrets) and
  `POST /api/llm-router/classifier/preview` (superuser, rate limited, side-effect free) in
  `server/src/llm_router/classifier.rs`, registered in OpenAPI; the preview reuses the
  router's own `ClassifierService` instance.
- A **Classification** tab on the Router page: effective backend (Regex, Jev or Laya) with
  availability and setup guidance, a query/context form with an explicit Test button,
  per-backend result cards next to the regex baseline, unconfigured backends shown as not
  run, and every loading/invalid/unavailable/unauthorized/timeout/error state. Mock variants
  for the regex-default, Jev-unconfigured, Laya-loaded, Laya-missing and failing deployments.
- Regenerated `schema.gen.ts` (classifier hunks only), MSW handlers, copy, error rules,
  16 component tests with axe, one Playwright case.

## Checks

`cargo fmt --check`, `cargo clippy -p nasiko-server --all-targets`, `cargo test -p
nasiko-server --lib`; `npm run typecheck`, `eslint --max-warnings 0`, `vitest run`,
`npm run build`, Playwright router cases; manual browser pass at desktop/phone,
light/dark, keyboard. Details and the pre-existing client drift in
`llm-router/docs/classifier/COMPANION.md`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
