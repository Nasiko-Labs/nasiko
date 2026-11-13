/**
 * Pure formatting for a build's captured output (`GET /api/builds/{id}/logs`): strip the ANSI colour
 * escapes build tools emit (cargo paints its errors red, which a browser would otherwise show as
 * `[91m` noise) and flag the lines that report a failure, so the viewer can paint those in the
 * theme's destructive colour. Kept out of the component file so fast refresh stays happy.
 */

/** ANSI SGR colour/style escapes (`ESC [ … m`) — stripped so they don't render as literal text. */
// eslint-disable-next-line no-control-regex -- ESC (U+001B) is exactly what an ANSI escape begins with
const ANSI = /\u001b\[[0-9;]*m/g

/**
 * Error-wrapper labels the control plane stacks in front of a failed build's output — the runtime's
 * RuntimeError Display ("internal error: ", "image not found: ") and the build worker's own prefixes
 * ("docker build: ", "build: ", "BUILD_FAILED:"). Stripped from the start so the log opens on the
 * first real build line, not "internal error: Step 1/12 …" (which would also falsely paint it red).
 */
const WRAPPER =
  /^(?:BUILD_FAILED:|docker build: |build: |internal error: |image not found: |backend unreachable: |resource conflict: )+/

/**
 * Lines that report a failure, highlighted in red. Anchored on whole words so an "errors: 0" summary
 * or a path with "failed" inside a word doesn't trip it; covers Docker ("COPY failed", "returned a
 * non-zero code", "file does not exist"), cargo/rustc (`error[E0432]`, `error:`) and common phrasing.
 */
const ERROR_LINE =
  /\b(error|failed|failure|fatal|panicked?|denied)\b|(?:^|\s)E\d{3,}\b|not found|no such|does not exist|non-zero/i

export interface BuildLogLine {
  text: string
  error: boolean
}

/** Split raw build output into display lines, ANSI stripped, each flagged if it reports a failure. */
export function buildLogLines(raw: string): BuildLogLine[] {
  return raw
    .replace(WRAPPER, '')
    .replace(ANSI, '')
    .replace(/\r$/gm, '')
    .split('\n')
    .map((text) => ({ text, error: ERROR_LINE.test(text) }))
}
