/**
 * The endpoint inventory (plans/feat-live-contract.md §7.4): the server paths the app's source calls, so a new call
 * without a manifest entry shows up. Regex scanning is brittle by nature, so the report is information until the
 * manifest's `strict` flag makes it a gate.
 */

const PATH_START = /^\/(api\/|health$|health\?|\.well-known\/)/

/** Same-file string constants holding a server path (`const OBS = '/api/observability'`). */
function constants(source: string): Map<string, string> {
  const out = new Map<string, string>()
  for (const m of source.matchAll(/const ([A-Za-z_$][\w$]*)\s*=\s*(['`])(\/[^'`$]*)\2/g)) {
    if (PATH_START.test(m[3]!)) out.set(m[1]!, m[3]!)
  }
  return out
}

/** Drop line and block comments (strings containing `//`, like URLs, are rare in path literals and kept). */
function stripComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`\\])\/\/.*$/gm, '$1')
}

/**
 * Server paths in one source file: string and template literals that start with /api/, /health or /.well-known/,
 * including templates that start with a same-file path constant (`${OBS}/agent/${id}/stats`). Interpolations become
 * `{}`; query strings are dropped.
 */
export function extractPaths(source: string): string[] {
  const consts = constants(stripComments(source))
  // A constant's own declaration is a prefix, not a call: only its uses count.
  const text = stripComments(source).replace(/const [A-Za-z_$][\w$]*\s*=\s*(['`])\/[^'`$]*\1/g, '')
  const out = new Set<string>()
  const add = (raw: string) => {
    const path = raw
      .split('?')[0]!
      .replace(/\$\{[^}]*\}/g, '{}')
      .replace(/\/+$/, '')
    if (PATH_START.test(path) || path === '/health') out.add(path)
  }
  for (const m of text.matchAll(/'(\/[^'\n]*)'|"(\/[^"\n]*)"/g)) add(m[1] ?? m[2]!)
  for (const m of text.matchAll(/`([^`]*)`/g)) {
    let t = m[1]!
    const lead = /^\$\{([A-Za-z_$][\w$]*)\}/.exec(t)
    if (lead && consts.has(lead[1]!)) t = consts.get(lead[1]!)! + t.slice(lead[0].length)
    if (t.startsWith('/')) add(t)
  }
  return [...out].sort()
}

/** A manifest path with its `{token}`s as wildcards (`/api/agents/{agent0}` covers `/api/agents/{}`). */
function pattern(manifestPath: string): RegExp {
  const esc = manifestPath.split(/\{[^}]+\}/).map((p) => p.replace(/[.*+?^$()|[\]\\]/g, '\\$&'))
  return new RegExp(`^${esc.join('[^/]+')}$`)
}

/**
 * Source paths no manifest path covers. Either side's placeholder matches a segment on the other: a manifest token
 * (`{agent0}`) covers a literal source segment, and a source interpolation (`{}`) covers a concrete manifest segment
 * (`/api/flows/000…`).
 */
export function uncovered(
  sourcePaths: readonly string[],
  manifestPaths: readonly string[],
): string[] {
  const patterns = manifestPaths.map(pattern)
  const concrete = manifestPaths.map((m) => m.replace(/\{[^}]+\}/g, 'X'))
  return sourcePaths.filter((p) => {
    const probe = p.replace(/\{\}/g, 'X')
    const fromSource = pattern(p.replace(/\{\}/g, '{x}'))
    return !patterns.some((r) => r.test(probe)) && !concrete.some((c) => fromSource.test(c))
  })
}
