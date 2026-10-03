/**
 * The one Vite config every edition builds from (docs/lab-vs-react-migration-review.md §10.6). Published with the
 * core: it never names a private edition. Each edition's own vite.config.ts passes its layers in.
 */
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import babel from '@rolldown/plugin-babel'
import tailwindcss from '@tailwindcss/vite'
import { tanstackRouter } from '@tanstack/router-plugin/vite'
import react, { reactCompilerPreset } from '@vitejs/plugin-react'
import { loadEnv, type Plugin, type UserConfig } from 'vite'

export const ROOT = dirname(fileURLToPath(import.meta.url))
export const COMMON_SRC = join(ROOT, 'common/src')

export interface AppOptions {
  /** 'oss', 'ee', …: the cache dir and the id the app's `src/edition.ts` declares. */
  id: string
  /** The edition app dir holding `src/` (index.html, main.tsx, edition.ts) and `dist/`. */
  appDir: string
  /** Layer `src/` dirs stacked on the core, lowest first (EE: its own src). */
  layers?: string[]
  /** One alias per layer (EE: `@ee`). */
  alias?: Record<string, string>
  mode: string
}

// The OSS nasiko-server. Proxying makes the browser see one origin, which is what the server expects: CORS is
// deny-by-default (empty CORS_ALLOWED_ORIGINS) and the login cookie is `HttpOnly; SameSite=Strict`.
const PROXIED = ['/api', '/health', '/a2a', '/.well-known']

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
    join(ROOT, 'common'),
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

export function nasikoApp(o: AppOptions): UserConfig {
  const env = loadEnv(o.mode, ROOT, '')
  const target = env.NASIKO_API_URL || 'http://localhost:8080'
  const src = join(o.appDir, 'src')
  const routes = shippedRoutes([
    join(COMMON_SRC, 'routes'),
    ...(o.layers ?? []).map((l) => join(l, 'routes')),
  ])
  return {
    root: src,
    envDir: ROOT,
    publicDir: join(ROOT, 'common/public'),
    cacheDir: join(ROOT, 'node_modules/.vite', o.id),
    plugins: [
      leakGuard(o),
      // Must run before react(): it generates the edition's src/routeTree.gen.ts from the core's routes.
      tanstackRouter({
        target: 'react',
        routesDirectory: join(COMMON_SRC, 'routes'),
        // A layer's own `routes/` (EE: `/weave`) mount beside the core's; `shippedRoutes` above already failed a
        // path served twice. Without layers the core's directory is read as is.
        ...(o.layers?.length
          ? {
              virtualRouteConfig: {
                type: 'root' as const,
                file: '__root.tsx',
                children: [COMMON_SRC, ...o.layers].map((dir) => ({
                  type: 'physical' as const,
                  pathPrefix: '',
                  directory: relative(join(COMMON_SRC, 'routes'), join(dir, 'routes')) || '.',
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
      // the biggest files), generated code and vendored Weave core (framework-free).
      babel({
        presets: [reactCompilerPreset()],
        exclude:
          /[\\/]node_modules[\\/]|[\\/]src[\\/](weave[\\/]core|mocks|test)[\\/]|\.test\.tsx?$|\.gen\.ts$/,
      }),
      tailwindcss(),
      routesManifest(routes),
      cspHashes(),
    ],
    resolve: {
      alias: {
        ...o.alias,
        '@edition': src,
        '@': COMMON_SRC,
      },
    },
    // Port 3000 is deliberate: gstack /qa only probes 3000, 4000 and 8080.
    server: {
      port: 3000,
      strictPort: true,
      proxy: Object.fromEntries(PROXIED.map((p) => [p, { target, changeOrigin: false }])),
    },
    preview: { port: 3000, strictPort: true },
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
                test: /node_modules[\\/](@visx|d3-|reduce-css-calc|reduce-function-call|math-expression-evaluator|balanced-match)|common[\\/]src[\\/]components[\\/]charts[\\/]/,
              },
            ],
          },
        },
      },
    },
  }
}
