/**
 * Agent names (plans/feat-deploy.md §4.1). The server checks an upload's `name` with `validate_version_tag`
 * (`agents/upload.rs`): 1–128 characters, starting with [a-zA-Z0-9_], then only [a-zA-Z0-9._-]. Pure.
 */

import { checkCopy } from './checkCopy'

export const NAME_MAX = 128

/** The problem with a name, in the server's terms, or null when it's valid. */
export function nameProblem(name: string): string | null {
  if (name.length === 0 || name.length > NAME_MAX) return checkCopy.nameLength(NAME_MAX)
  if (!/^[a-zA-Z0-9_]/.test(name)) return checkCopy.nameStart
  if (!/^[a-zA-Z0-9._-]+$/.test(name)) return checkCopy.nameChars
  return null
}

/** A valid name from a file name: `My Agent (v2).zip` → `My-Agent-v2`. Empty when nothing usable is left. */
export function nameFromFile(fileName: string): string {
  const base = fileName.replace(/\.zip$/i, '')
  const cleaned = base
    .replace(/[^a-zA-Z0-9._-]+/g, '-')
    .replace(/-{2,}/g, '-')
    .replace(/^[^a-zA-Z0-9_]+/, '')
    .replace(/[-.]+$/, '')
  return cleaned.slice(0, NAME_MAX)
}

/**
 * The terminal alternative on the Upload and GitHub tabs, checked against recorded `nasiko upload --help`. Only a valid
 * agent name goes into it (never shell characters from a link or a half-typed field); otherwise the placeholder.
 */
export const uploadCommand = (dir: string) =>
  `nasiko upload ./${nameProblem(dir) === null ? dir : checkCopy.placeholderDir}`
