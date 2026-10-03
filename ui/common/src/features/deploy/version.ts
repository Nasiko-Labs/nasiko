/**
 * Agent versions are strict `x.y.z` on the server (`upload.rs` `validate_version`); pure helpers, tested in logic.test.ts.
 */

export interface SemVer {
  major: number
  minor: number
  patch: number
}

export function parseVersion(v: string | null | undefined): SemVer | null {
  // semver's core rule, like the server's `parse_plain_version`: no leading zeros, safe integers.
  const m = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.exec((v ?? '').trim())
  if (!m || [m[1], m[2], m[3]].some((n) => !Number.isSafeInteger(Number(n)))) return null
  return { major: Number(m[1]), minor: Number(m[2]), patch: Number(m[3]) }
}

/** The next patch of `v`, or null when `v` isn't x.y.z. */
export function nextPatch(v: string | null | undefined): string | null {
  const p = parseVersion(v)
  return p ? `${p.major}.${p.minor}.${p.patch + 1}` : null
}

export interface VersionConflict {
  version: string
  suggested: string
  message: string
}

/**
 * `github.rs` writes a clone's version clash into the upload status as
 * `VERSION_CONFLICT:<ver>:<suggested>:<message>`; anything else (or a malformed tail) is not a conflict.
 */
export function parseVersionConflict(detail: string | null | undefined): VersionConflict | null {
  const [, version = '', suggested = '', message = ''] =
    /^VERSION_CONFLICT:([^:]+):([^:]+):(.*)$/s.exec(detail ?? '') ?? []
  if (!parseVersion(version) || !parseVersion(suggested)) return null
  return { version, suggested, message: message.trim() }
}

/** An upload's 409 body: `version X already exists in this agent's history — choose a new version`. */
export function versionFromConflictText(text: string | null | undefined): string | null {
  return /version (\d+\.\d+\.\d+) already exists/.exec(text ?? '')?.[1] ?? null
}
