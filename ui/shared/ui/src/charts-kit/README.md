# charts-kit — the vendored visx chart kit

The animated, visx-based chart kit that TokenOps composes its charts from (month hero, timeline,
day panel, attribution). It is a **separate lazy entry** of `@nasiko/ui`
(`@nasiko/ui/charts-kit`), next to `@nasiko/ui/charts` (Recharts); import it only behind a
dynamic import boundary.

## Provenance

- **Origin**: Bklit UI (<https://github.com/bklit/bklit-ui>, shadcn registry at
  <https://ui.bklit.com>), MIT — see [`NOTICE`](./NOTICE).
- **Via**: nasiko-cloud-rs-genui (branch `feat/genui-chart-kit`, commit `c0c14a5b`), then the
  React migration's kit, then **nasiko-ui-lab** commit `1b99bdda`,
  `common/src/components/charts/` — the version vendored here. Lab edits are marked `Lab:` in the
  source and stay as they are.
- Local edit (TokenOps port): every `motion.<tag>` element is `m.<tag>` from `motion/react-m` (same
  components, features loaded by the app's `LazyMotion`), because `MotionProvider` renders
  `LazyMotion` in `strict` mode, which throws on a `motion` element. The five `tooltip/` files were
  missed at first (hovering a chart threw in development); they were converted on 2026-10-08 and the
  base `no-restricted-imports` rule now applies to this directory, so an eager `motion` import cannot
  come back. `cost-scatter.tsx` is the lab's `features/tokenops/components/CostScatter.tsx` drawing,
  moved here (see its header).
- `lib/format.ts` is `common/src/lib/format.ts` of the same lab commit (verbatim);
  `lib/series.ts` is its `common/src/lib/chart.ts` (`seriesAt`, `OTHER_SERIES`, verbatim).
- **ADR-0003 amendment (2026-10-08, proposed)** admits visx/d3 inside this directory only
  (`tooling/architecture`: layer `charts-kit`, `CHART_ENGINE_PATTERNS`, dependency-cruiser
  `chart-engine-only-in-charts-kit`, fixture `visx-outside-kit`); Recharts stays the catalog's engine.

## Destination edits (behaviour)

Unlike the toolchain edits under "Fidelity rule", these change what the kit does, to obey this
repository's motion policy (ADR-0006). Each file says so in a `Destination:` header line.

- **Motion policy** (`use-kit-reduced-motion.ts`, new): the kit's nine `useReducedMotion()` call sites
  (`bar`, `series-bar`, `chart-reveal-clip`, `cost-scatter`, `loading-sweep` ×2, `shimmering-text`,
  `use-animated-series-path`, `use-animated-y-domains`, `use-grid-shimmer`) call `useKitReducedMotion()`
  instead. It is the lab's OS check, OR'd with the enclosing `MotionProvider`'s resolved mode: `none` or
  `reduced` (a surface `profile="none"`, a parent clamped to `none`/`reduced`, the user's preference)
  takes the kit's own at-rest path, the one the lab already draws for OS reduced motion. Outside any
  provider the lab's rule is unchanged. Two lab `=== true` / `!== true` comparisons became plain
  boolean tests (the adapter returns a boolean). Tests: `motion-policy.test.tsx`,
  `motion-policy-os-reduced.test.tsx`, and `tests/motion/chart-kit.motion.ts` (first painted frame).
- **Tooltips** (`tooltip/*.tsx`): `motion.<tag>` → `m.<tag>`, see above.

What still moves under `reduced`/`none`, by design or as a known gap: the BarChart keeps its reveal
timer (interaction opens after `animationDuration` + stagger even when the bars are drawn at rest — the
lab does the same under OS reduced motion); tooltip springs (`useSpring`) follow the pointer in every
mode. Under `none` the host's `MotionConfig skipAnimations` makes every `m` element's animations
instant, which covers the kit's axes, grid and markers that do not consult the hook.

## Fidelity rule

The kit must render and animate exactly as it does in the lab. Edits made while vendoring are
limited to what this repository's toolchain requires and never change behaviour:

- `"use client"` on every module that uses React hooks, context or the DOM (the lab's Vite SPA
  did not need it; Next.js does). `'use no memo'` directives stay, after `'use client'`.
- Imports: `@/lib/utils` → `../lib/cn` (the `cn` package; none of the kit's class strings use
  the lab's custom tailwind-merge groups, so merging is identical), `@/lib/format` →
  `./lib/format`.
- `exactOptionalPropertyTypes` (on here, off in the lab): every optional property of the kit's
  own types reads `prop?: T | undefined`, and the few props handed to motion/visx that cannot
  take `undefined` are spread conditionally instead (same props reach the element). The default
  ease is a typed constant (`DEFAULT_CHART_ENTER_EASE`) so `ease` is never `undefined`.
- Lint: a handful of `eslint-disable-next-line <rule> -- <reason>` lines for this repo's
  `strictTypeChecked` set, and one override block in `shared/ui/eslint.config.mjs` scoped to
  this directory (`no-unnecessary-condition` off; unused lab
  directives not reported). The dependency-cruiser cycle rule lets this directory have
  type-only cycles (`chart-context.ts` ↔ scale helpers), never runtime ones.
- Formatting: this repository's Prettier config (double quotes, semicolons), so `check:format`
  passes. Re-vendor by formatting the lab copy the same way before diffing.

## Re-vendoring

1. Copy `common/src/components/charts/` from the lab, plus `lib/format.ts` and `lib/chart.ts`.
2. Re-apply the edits above (they are mechanical; the typed-ease and conditional-spread sites
   are listed in the vendoring report in the PR that landed this directory), then the
   destination edits (`grep -rl "Destination:" .` lists the files).
3. `pnpm --filter @nasiko/ui exec eslint src/charts-kit`, `tsc --noEmit -p .`,
   `pnpm exec vitest run shared/ui/src/charts-kit`, `node tooling/architecture/check.mjs`,
   the `charts-kit-line--two-series` story under the browser suite (axe, light and dark), and
   `pnpm test:motion` (`chart-kit.motion.ts`: at rest under `none`/`reduced`, grows otherwise).
