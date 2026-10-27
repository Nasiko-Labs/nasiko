/**
 * zod 4 builds a fast path for each object schema with `new Function` when it may, and probes for that as the schema
 * is created: under the served CSP (no 'unsafe-eval', oss/spa `build_csp`) the probe alone is reported as a violation.
 * Its `jitless` switch must be on before the first schema exists, and schemas are created as their modules load, in
 * whichever chunk the bundler put them, so no module is early enough: vite.shared.ts puts this classic inline script
 * in every page's head instead. zod adopts the global as its config, the object `z.config()` writes
 * (zod/v4/core/core.js); zodJitless.test.ts checks that it still does. Import-free: vite.shared.ts loads it in Node.
 * After codex/nasiko-react-foundation ui/shared/app-core/src/zod-jitless.ts, which set it from main.tsx.
 */
export const ZOD_JITLESS_SCRIPT = 'globalThis.__zod_globalConfig={jitless:true}'
