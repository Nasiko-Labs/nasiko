# Nasiko React SPA

React + Vite + TanStack Router, with Rust backend authority. No Next, SSR or production frontend Node service. This migration is not production acceptance. Desktop work is out of scope.

Read [architecture](docs/architecture/README.md), [ADR-0008](docs/adr/0008-react-spa-no-ssr.md), [ADR-0009](docs/adr/0009-product-structure-editions-profiles.md) and [migration evidence](docs/migration/spa-2026-10-12.md).

Layout (ADR-0009): `shared/` publication-approved packages · `oss/app` the OSS root · `ee/app` the full EE product ·
`ee/features` private feature packages (Weave included) · `ee/tenancy` profiles and tenancy · `ee/registry` the
Registry's own root and packages. The package ledger is `tooling/editions/ownership.json` + `ee/ownership.json`.

```sh
export OPENUI_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1 STORYBOOK_DISABLE_TELEMETRY=1
npx --yes pnpm@12.8.1 install --frozen-lockfile
npx --yes pnpm@12.8.1 dev                 # OSS app, API proxy defaults to http://127.0.0.1:8082
npx --yes pnpm@12.8.1 --filter @nasiko-ee/workspace dev
npx --yes pnpm@12.8.1 check
npx --yes pnpm@12.8.1 test
npx --yes pnpm@12.8.1 build               # both static edition outputs
```

Node range and exact tool versions live in package.json/lockfile. Set `NASIKO_API_UPSTREAM` for a local Rust backend. `VITE_*` values are public build-time configuration, never secrets. Mock mode is explicit in `dev:mock` or test builds and is rejected in release builds. Central dashboard mode is not yet admitted.

From the repository root, `just build-react-server oss` or `just build-react-server ee` builds the selected Rust release binary with its SPA. It does not deploy. See [delivery and qualification](docs/architecture/11-unified-installation.md).

Browser suites require a Playwright-compatible Chromium. An installed browser can be selected with `PW_EXECUTABLE`. `pnpm test:e2e`, `test:motion`, `test:browser`, `test:weave`, `check:bundles` and `check:oss-export` are separate checks. The publication boundary: `pnpm check:boundaries`, `pnpm publication:check`, `pnpm check:oss-export [--check] [--rust]`. Preserved historical reports do not certify the current revision.
