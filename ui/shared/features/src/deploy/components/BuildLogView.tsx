/**
 * Renders a build's captured output (`GET /api/builds/{id}/logs`): the whole log on success and on
 * failure alike. On a failed build the lines that mark what went wrong are called out in the
 * destructive color; on a successful build nothing is reddened, since build output routinely
 * mentions "error"/"failed" in benign places (a crate named `error-chain`, an "errors: 0" summary).
 * Formatting (ANSI stripping, error-line detection) is pure in `../buildLog`.
 */
import { useMemo } from 'react'
import { cn } from '@nasiko/ui/lib/utils'
import { buildLogLines } from '../buildLog'

export function BuildLogView({
  logs,
  className,
  highlightErrors = true,
}: {
  logs: string
  className?: string
  /** Paint failure lines in the destructive color. Pass the build's failed state: a successful
   * build should never show red lines, whatever its output happens to contain. */
  highlightErrors?: boolean
}) {
  const lines = useMemo(() => buildLogLines(logs), [logs])
  return (
    <pre
      className={cn(
        'max-h-96 overflow-auto rounded-md border bg-muted/40 p-2 font-mono break-words whitespace-pre-wrap',
        className,
      )}
    >
      {lines.map((line, i) => (
        // Build output is ordered and static, so a line's position is a stable key.
        // eslint-disable-next-line @eslint-react/no-array-index-key -- log lines never reorder
        <span key={i} className={cn('block', highlightErrors && line.error && 'text-destructive')}>
          {line.text || ' '}
        </span>
      ))}
    </pre>
  )
}
