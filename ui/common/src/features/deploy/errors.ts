/**
 * Server error text → what went wrong and how to fix it (plans/feat-deploy.md §4.1, §5). Pure; tested in logic.test.ts.
 *
 * The strings are the server's own (nasiko-cloud-rs `2d6178e4`): `agents/upload.rs` (`validate_agent_zip`, the 400/409/413
 * bodies and the upload status `error_message`) and `github.rs` (clone). Server errors are plain text.
 */
import { copy } from './copy'
import { nextPatch, parseVersionConflict, versionFromConflictText } from './version'

/** A checklist item on the Upload tab (design review 4: a 400 turns the item it names red). */
export type ChecklistItem = 'dockerfile' | 'entrypoint' | 'version' | 'size'

export interface BuildError {
  problem: string
  fix: string
  /** The checklist item the error is about, when it is one. */
  item?: ChecklistItem
  /** A version clash: the next version to offer ("Deploy as vX", design review 3). */
  suggested?: string
  /** The version that clashed. */
  version?: string
}

const RULES: readonly { test: RegExp; error: Omit<BuildError, 'suggested' | 'version'> }[] = [
  {
    test: /no Dockerfile found in root of zip/i,
    error: { ...copy.errors.noDockerfile, item: 'dockerfile' },
  },
  {
    test: /Dockerfile has no FROM instruction/i,
    error: { ...copy.errors.noFrom, item: 'dockerfile' },
  },
  {
    test: /no Python entrypoint found/i,
    error: { ...copy.errors.noEntrypoint, item: 'entrypoint' },
  },
  {
    test: /version_tag is required|no valid version found|must be in x\.y\.z format/i,
    error: { ...copy.errors.noVersion, item: 'version' },
  },
  {
    test: /upload exceeds 100 MiB|exceeds 100 MB/i,
    error: { ...copy.errors.tooLarge, item: 'size' },
  },
  // oss/utils/src/zip.rs extract_zip_reader: every entry counts, directories and __MACOSX included.
  {
    test: /zip contains \d+ files, limit is|zip uncompressed size exceeds/i,
    error: { ...copy.errors.tooBig, item: 'size' },
  },
  { test: /GitHub not connected/i, error: copy.errors.githubDisconnected },
  { test: /git clone failed/i, error: copy.errors.cloneFailed },
]

/** Map one server error string (a response body or an upload status `error_details[0]`) to a problem and a fix. */
export function explainError(detail: string | null | undefined): BuildError {
  const conflict = parseVersionConflict(detail)
  if (conflict)
    return {
      ...copy.errors.versionConflict(conflict.version),
      version: conflict.version,
      suggested: conflict.suggested,
    }
  const clashed = versionFromConflictText(detail)
  if (clashed) {
    const suggested = nextPatch(clashed) ?? undefined
    return {
      ...copy.errors.versionConflict(clashed),
      item: 'version',
      version: clashed,
      ...(suggested ? { suggested } : {}),
    }
  }
  const rule = RULES.find((r) => r.test.test(detail ?? ''))
  return rule ? { ...rule.error } : { ...copy.errors.generic }
}

/** A one-line failure reason for a Builds row (design review 2): the problem, never raw server text. */
export function failureReason(details: readonly string[] | null | undefined): string {
  return explainError(details?.[0]).problem
}
