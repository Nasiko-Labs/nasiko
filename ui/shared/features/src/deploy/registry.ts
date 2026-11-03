/**
 * Registry references (plans/feat-deploy.md §4.3). `catalog/import.rs` `import_registry` expects
 * `registry.host/owner/name[:tag]` (HTTPS only, no registry login) and 422s a host outside its allowed list
 * (`registry.nasiko.dev`, `REGISTRY_IMPORT_ALLOWED_HOSTS`, the server's own registry). Pure.
 */

export const DEFAULT_REGISTRY = 'registry.nasiko.dev'

export interface ParsedReference {
  host: string
  repo: string
  tag: string
}

/** What the server gets: the reference without a scheme (`import_registry` splits the host at the first `/`). */
export const canonicalReference = (input: string) => input.trim().replace(/^https?:\/\//, '')

/**
 * The parts of a reference, or null when it isn't `host/owner/name[:tag]`. The host is Docker's rule: it has a dot or a
 * port, or is `localhost` (so a compose registry like `registry:5000` works); the server's allow list decides the rest (422).
 */
export function parseReference(input: string): ParsedReference | null {
  const ref = canonicalReference(input)
  const m =
    /^([a-z0-9.-]+(?::\d+)?)\/([a-z0-9._-]+(?:\/[a-z0-9._-]+)+)(?::([\w.-]{1,128}))?$/i.exec(ref)
  if (!m) return null
  const [, host = '', repo = '', tag = 'latest'] = m
  if (!host.includes('.') && !host.includes(':') && host !== 'localhost') return null
  return { host, repo, tag }
}
