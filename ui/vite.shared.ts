/**
 * The one Vite config every edition builds from (docs/lab-vs-react-migration-review.md §10.6). Published with the
 * core: it never names a private edition. Each edition's own vite.config.ts passes its layers in.
 */
import { createHash } from 'node:crypto'
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import { dirname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import babel from '@rolldown/plugin-babel'
import tailwindcss from '@tailwindcss/vite'
import { tanstackRouter } from '@tanstack/router-plugin/vite'
import react, { reactCompilerPreset } from '@vitejs/plugin-react'
import { loadEnv, type Plugin, type UserConfig } from 'vite'
import { precompress } from './scripts/lib/precompress.ts'
import { BOOT_GUARD_SCRIPT } from './shared/app-core/src/staleBuildBoot.ts'
import { ZOD_JITLESS_SCRIPT } from './shared/app-core/src/zodJitless.ts'

export const ROOT = dirname(fileURLToPath(import.meta.url))
/** The shared packages, `shared/<name>/src`, each imported as `@nasiko/<name>/…`. */
export const SHARED = join(ROOT, 'shared')
/** The core's routes, which every control-plane edition serves. */
export const CORE_ROUTES = join(SHARED, 'app-shell/src/routes')
/**
 * One alias per shared package, to its source: a package is never built on its own, the app's build compiles it. An
 * alias rather than package.json `exports`: TypeScript resolves an extensionless specifier through `paths`
 * (tsconfig.base.json lists the same), not through an `exports` pattern.
 */
export const PACKAGE_ALIASES: Record<string, string> = Object.fromEntries(
  readdirSync(SHARED)
    .filter((d) => existsSync(join(SHARED, d, 'src')))
    .map((d) => [`@nasiko/${d}`, join(SHARED, d, 'src')]),
)

export interface AppOptions {
  /** 'oss', 'ee', …: the cache dir and the id the app's `src/edition.ts` declares. */
  id: string
  /** The edition app dir holding `src/` (index.html, main.tsx, edition.ts) and `dist/`. */
  appDir: string
  /** Layer `src/` dirs stacked on the core, lowest first (EE: its own src). */
  layers?: string[]
  /** One alias per layer package (an edition's feature packages, beside its app). */
  alias?: Record<string, string>
  mode: string
  /**
   * false: the app serves only its own `src/routes` (with its own `__root.tsx`), none of the core's. For an app that
   * uses the core's components but is not the control plane. Default true.
   */
  commonRoutes?: boolean
  /** The dev server's API paths and the server they go to. Default: the control plane's (`PROXIED`, NASIKO_API_URL). */
  proxy?: { paths: readonly string[]; target: string }
  /** Dev and preview port. Default 3000. */
  port?: number
}

// The OSS nasiko-server. Proxying makes the browser see one origin, which is what the server expects: CORS is
// deny-by-default (empty CORS_ALLOWED_ORIGINS) and the login cookie is `HttpOnly; SameSite=Strict`.
const PROXIED = ['/api', '/health', '/a2a', '/.well-known'] as const

/**
 * The dev proxy key for an API path: a whole path segment, as a regex (Vite reads a key starting with `^` as one). A
 * plain key is a string prefix, so `/api` would also take `/api.ts`, the URL Vite serves an app's own `src/api.ts` at.
 */
export function proxyKey(path: string): string {
  return `^${path.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?=[/?#]|$)`
}

/** URL path of a file-based route file under a routes dir (`_app/agents.$agentId.tsx` → `/agents/$agentId`). */
export function routePath(file: string): string {
  const segments = file
    .replace(/\.tsx$/, '')
    .split(/[/\\]/)
    .flatMap((s) => s.split('.'))
    .filter((s) => !s.startsWith('_') && s !== 'index' && s !== 'route')
  return `/${segments.join('/')}`
}

/** Every route file under `dir`, relative, skipping `-`-prefixed files and the root route. */
export function routeFiles(dir: string): string[] {
  if (!existsSync(dir)) return []
  const out: string[] = []
  const walk = (d: string) => {
    for (const f of readdirSync(d)) {
      const p = join(d, f)
      if (statSync(p).isDirectory()) walk(p)
      else if (
        f.endsWith('.tsx') &&
        !f.startsWith('-') &&
        f !== '__root.tsx' &&
        !f.includes('.test.')
      )
        out.push(relative(dir, p))
    }
  }
  walk(dir)
  return out.sort()
}

/**
 * A layer adds routes but never overrides a core one (plan §10.4): a path served twice fails the build.
 * Returns the edition's shipped paths (what the server may answer with index.html).
 */
export function shippedRoutes(routeDirs: string[]): string[] {
  const seen = new Map<string, string>()
  for (const dir of routeDirs) {
    for (const file of routeFiles(dir)) {
      if (file.endsWith('_app.tsx') || /(^|[/\\])_[^/\\]+\.tsx$/.test(file)) continue // pathless layouts
      const path = routePath(file)
      const other = seen.get(path)
      if (other) throw new Error(`Route ${path} is defined twice: ${other} and ${join(dir, file)}`)
      seen.set(path, join(dir, file))
    }
  }
  return [...seen.keys()].sort()
}

/** Fails any import that resolves outside this edition's allowed roots: EE code can never reach an OSS build. */
function leakGuard(o: AppOptions): Plugin {
  const allowed = [
    SHARED,
    o.appDir,
    ...(o.layers ?? []),
    // The live-contract tests drive the recorder and seed scripts; nothing in an app bundle may.
    ...(o.mode === 'test' ? [join(ROOT, 'scripts')] : []),
  ]
  return {
    name: 'nasiko:leak-guard',
    enforce: 'pre',
    async resolveId(source, importer, opts) {
      const hit = await this.resolve(source, importer, { ...opts, skipSelf: true })
      const id = hit?.id.split('?')[0]
      // Dependencies are allowed wherever they resolve (a symlinked or shared node_modules).
      if (
        !id ||
        !id.startsWith('/') ||
        id.startsWith('/@') ||
        id.includes('\0') ||
        id.includes(`${sep}node_modules${sep}`)
      )
        return hit
      if (!allowed.some((root) => id === root || id.startsWith(root + sep))) {
        this.error(`${source} (from ${importer}) resolves outside the ${o.id} edition: ${id}`)
      }
      return hit
    },
  }
}

/**
 * `<file>.br` and `<file>.gz` beside this build's own text output (scripts/lib/precompress.ts), which the Rust servers
 * serve by `Accept-Encoding` (oss/spa). `writeBundle`: the files Vite wrote, nothing another plugin adds later (EE's
 * genui frame keeps its runtime compression).
 */
function precompressed(): Plugin {
  return {
    name: 'nasiko:precompress',
    apply: 'build',
    writeBundle(options, bundle) {
      const dir = options.dir
      if (dir) precompress(Object.keys(bundle).map((file) => join(dir, file)))
    },
  }
}

/** `dist/routes.json`: the page manifest the Rust server reads to decide which paths get index.html. */
function routesManifest(routes: string[]): Plugin {
  let out = ''
  return {
    name: 'nasiko:routes-manifest',
    apply: 'build',
    configResolved(c) {
      out = c.build.outDir
    },
    closeBundle() {
      mkdirSync(out, { recursive: true })
      writeFileSync(join(out, 'routes.json'), `${JSON.stringify({ routes }, null, 2)}\n`)
    },
  }
}

/**
 * `dist/nasiko-spa.json`: whether this build can answer API calls from the mock seed. A production build honours mock
 * mode or partial mocks only under VITE_NASIKO_ALLOW_MOCK_BUILD (shared/app-core/src/env.ts `readEnv`, which this can't
 * import: it reads `import.meta.env` as it loads). Each server's build.rs refuses such a bundle in a release binary. A
 * live build also drops MSW's worker, which nothing in it registers.
 */
function apiMode(): Plugin {
  let out = ''
  let mocks = false
  return {
    name: 'nasiko:api-mode',
    apply: 'build',
    configResolved(c) {
      out = c.build.outDir
      const env = c.env as Record<string, string | undefined>
      mocks =
        env.VITE_NASIKO_ALLOW_MOCK_BUILD === 'true' &&
        (env.VITE_NASIKO_API_MODE?.trim() === 'mock' || !!env.VITE_NASIKO_MOCK?.trim())
    },
    closeBundle() {
      writeFileSync(
        join(out, 'nasiko-spa.json'),
        `${JSON.stringify({ apiMode: mocks ? 'mock' : 'live' })}\n`,
      )
      if (!mocks) rmSync(join(out, 'mockServiceWorker.js'), { force: true })
    },
  }
}

/** The sha256 source list of an HTML page's inline scripts, for a CSP `script-src` without 'unsafe-inline'. */
export function inlineScriptHashes(html: string): string[] {
  return [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/g)].map(
    (m) =>
      `'sha256-${createHash('sha256')
        .update(m[1] ?? '')
        .digest('base64')}'`,
  )
}

/**
 * `dist/csp.json`: the hashes of index.html's inline scripts (the pre-paint theme script, plus OSS's Reo loader) for
 * the server's CSP (plan §8 Phase 9). The page has no other inline script or style attribute.
 */
function cspHashes(): Plugin {
  let out = ''
  return {
    name: 'nasiko:csp-hashes',
    apply: 'build',
    configResolved(c) {
      out = c.build.outDir
    },
    closeBundle() {
      const html = readFileSync(join(out, 'index.html'), 'utf8')
      writeFileSync(
        join(out, 'csp.json'),
        `${JSON.stringify({ 'script-src': inlineScriptHashes(html) }, null, 2)}\n`,
      )
    },
  }
}

/**
 * Reo (product analytics) reports from built bundles only: the dev server, and with it every e2e and sweep run, drops
 * the loader between index.html's "Start/End of Reo Javascript" comments, so no local session counts as a visit.
 */
function analyticsInBuildsOnly(): Plugin {
  return {
    name: 'nasiko:analytics-in-builds-only',
    apply: 'serve',
    transformIndexHtml: (html) =>
      html.replace(/\s*<!-- Start of Reo Javascript -->[\s\S]*?<!-- End of Reo Javascript -->/, ''),
  }
}

/** zod's `jitless` switch (shared/app-core/src/zodJitless.ts), inline after `<title>` so it runs before any module does. */
function zodJitless(): Plugin {
  return {
    name: 'nasiko:zod-jitless',
    transformIndexHtml: (html) =>
      html.replace('</title>', `</title>\n    <script>${ZOD_JITLESS_SCRIPT}</script>`),
  }
}

/**
 * The stale-build boot guard (shared/app-core/src/staleBuildBoot.ts), inline right after `<title>`: after `<meta charset>`
 * (the shell is served without a charset, so it must stay in the first 1024 bytes) and before the entry module and the
 * stylesheets, whose load errors it catches. `cspHashes` hashes it with the page's other inline scripts.
 */
function bootGuard(): Plugin {
  return {
    name: 'nasiko:boot-guard',
    apply: 'build',
    transformIndexHtml(html) {
      if (!html.includes('</title>'))
        throw new Error('index.html needs a <title> for the boot guard to follow')
      return html.replace('</title>', `</title>\n    <script>${BOOT_GUARD_SCRIPT}</script>`)
    },
  }
}

export function nasikoApp(o: AppOptions): UserConfig {
  const env = loadEnv(o.mode, ROOT, '')
  const proxy = o.proxy ?? { paths: PROXIED, target: env.NASIKO_API_URL || 'http://localhost:8080' }
  const port = o.port ?? 3000
  const src = join(o.appDir, 'src')
  const ownRoutes = o.commonRoutes === false
  const routes = shippedRoutes(
    ownRoutes
      ? [join(src, 'routes')]
      : [CORE_ROUTES, ...(o.layers ?? []).map((l) => join(l, 'routes'))],
  )
  return {
    root: src,
    envDir: ROOT,
    publicDir: join(SHARED, 'ui/public'),
    cacheDir: join(ROOT, 'node_modules/.vite', o.id),
    plugins: [
      leakGuard(o),
      // Must run before react(): it generates the edition's src/routeTree.gen.ts from the core's routes.
      tanstackRouter({
        target: 'react',
        routesDirectory: ownRoutes ? join(src, 'routes') : CORE_ROUTES,
        // A layer's own `routes/` (EE: `/weave`) mount beside the core's; `shippedRoutes` above already failed a
        // path served twice. Without layers the core's directory (or the app's own) is read as is.
        ...(o.layers?.length && !ownRoutes
          ? {
              virtualRouteConfig: {
                type: 'root' as const,
                file: '__root.tsx',
                children: [CORE_ROUTES, ...o.layers.map((l) => join(l, 'routes'))].map((dir) => ({
                  type: 'physical' as const,
                  pathPrefix: '',
                  directory: relative(CORE_ROUTES, dir) || '.',
                })),
              },
            }
          : {}),
        generatedRouteTree: join(src, 'routeTree.gen.ts'),
        autoCodeSplitting: true,
        // Loaders get their own chunk too: they import feature query code, which must not land in the shell.
        codeSplittingOptions: {
          defaultBehavior: [
            ['loader'],
            ['component'],
            ['pendingComponent'],
            ['errorComponent'],
            ['notFoundComponent'],
          ],
        },
      }),
      react(),
      // The React Compiler memoises components and hooks (plan §2, Phase 4). eslint-plugin-react-hooks' compiler rules
      // keep code compilable; a file that can't follow them opts out with 'use no memo' (the chart kit's animation
      // code). Not compiled: tests, test helpers and mocks (they only drive the app; skipping them also keeps Babel off
      // the biggest files), generated code and the vendored GenUI language runtime.
      babel({
        presets: [reactCompilerPreset()],
        exclude:
          /[\\/]node_modules[\\/]|[\\/]shared[\\/](mocks|testing)[\\/]|[\\/]src[\\/](genui[\\/]lang|mocks|test)[\\/]|\.test\.tsx?$|\.gen\.ts$/,
      }),
      tailwindcss(),
      routesManifest(routes),
      precompressed(),
      apiMode(),
      analyticsInBuildsOnly(),
      zodJitless(),
      bootGuard(),
      cspHashes(),
    ],
    resolve: {
      alias: {
        ...PACKAGE_ALIASES,
        ...o.alias,
        '@edition': src,
      },
    },
    // Port 3000 is deliberate: gstack /qa only probes 3000, 4000 and 8080.
    server: {
      port,
      strictPort: true,
      proxy: Object.fromEntries(
        proxy.paths.map((p) => [proxyKey(p), { target: proxy.target, changeOrigin: false }]),
      ),
    },
    preview: { port, strictPort: true },
    build: {
      outDir: join(o.appDir, 'dist'),
      emptyOutDir: true,
      // scripts/check-budgets.ts reads it (dist/.vite/manifest.json; not served, the Rust embed skips dotfiles).
      manifest: true,
      // Off in every edition (Vite's default, made explicit): a map would publish private EE source under /assets.
      sourcemap: false,
      rolldownOptions: {
        output: {
          // visx (+ d3) is most of the chart pages' weight; its own chunk keeps it cacheable across page changes.
          // Shared deps (React) must not be dragged in, or the entry would import the charts chunk. So @visx/text's own
          // CommonJS deps are listed: left out, rolldown parks them in a route chunk that imports charts, and the cycle
          // breaks every page (check-budgets.ts fails on any chunk cycle).
          codeSplitting: {
            includeDependenciesRecursively: false,
            groups: [
              {
                name: 'charts',
                test: /node_modules[\\/](@visx|d3-|reduce-css-calc|reduce-function-call|math-expression-evaluator|balanced-match)|shared[\\/]ui[\\/]src[\\/]charts[\\/]/,
              },
            ],
          },
        },
      },
    },
  }
}
