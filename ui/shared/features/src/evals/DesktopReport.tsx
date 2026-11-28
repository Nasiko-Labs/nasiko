import { useEffect, useRef, useState } from 'react'
import { z } from 'zod/mini'
import { Button } from '@nasiko/ui/primitives/button'
import { Input } from '@nasiko/ui/primitives/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import {
  Table,
  TableCaption,
  TableRow,
  TableHead,
  TableHeader,
  TableBody,
  TableCell,
} from '@nasiko/ui/primitives/table'
import { fmtMoneyFine } from '@nasiko/ui/lib/format'

const attempt = z.object({
  id: z.string(),
  task: z.string(),
  harness: z.string(),
  model: z.string(),
  trial: z.number(),
  outcome: z.string(),
  resolved: z.nullable(z.boolean()),
  seconds: z.number(),
  screenshots: z.array(z.string()),
  router_usd: z.nullable(z.number()),
  usage_status: z.string(),
})
const schema = z.object({
  version: z.literal(1),
  run_id: z.string(),
  tasks: z.array(z.object({ id: z.string(), prompt: z.string() })),
  attempts: z.array(attempt),
})
type Report = z.infer<typeof schema>
type Bundle = { report: Report; images: Map<string, File> }

async function readBundle(files: readonly File[]): Promise<Bundle> {
  const rootFile =
    files.find(
      (f) => f.name === 'report.json' && (f.webkitRelativePath ?? '').split('/').length === 2,
    ) ?? (files.length === 1 ? files[0] : undefined)
  if (!rootFile) throw new Error('Choose the output folder containing report.json.')
  const parsed = schema.safeParse(
    JSON.parse(
      await new Promise<string>((resolve, reject) => {
        const reader = new FileReader()
        reader.onload = () => resolve(String(reader.result))
        reader.onerror = () => reject(new Error('Could not read the report file.'))
        reader.readAsText(rootFile)
      }),
    ),
  )
  if (!parsed.success) throw new Error('This file is not a version 1 desktop eval report.')
  const images = new Map<string, File>()
  for (const f of files) {
    // Only files within the selected report directory can become image sources.
    const relative = (f.webkitRelativePath ?? '').split('/').slice(1).join('/')
    if (relative.endsWith('.png')) images.set(relative, f)
  }
  return { report: parsed.data, images }
}

function Screenshot({ file, index, count }: { file: File; index: number; count: number }) {
  const img = useRef<HTMLImageElement>(null)
  useEffect(() => {
    const next = URL.createObjectURL(file.slice(0, file.size, 'image/png'))
    if (img.current) img.current.src = next
    return () => URL.revokeObjectURL(next)
  }, [file])
  return (
    <img
      className="max-h-80 max-w-full rounded border object-contain"
      ref={img}
      alt={`Screenshot ${index + 1} of ${count}`}
    />
  )
}

const grade = (resolved: boolean | null) =>
  resolved === null ? 'Unverified' : resolved ? 'Passed' : 'Failed'

function ReportDetails({ report, images }: Bundle) {
  const [taskIndex, setTaskIndex] = useState(0)
  const [attemptIndex, setAttemptIndex] = useState(0)
  const [frame, setFrame] = useState(0)
  const task = report.tasks[taskIndex]
  const rows = report.attempts.filter((r) => r.task === task?.id)
  const selected = rows[attemptIndex]
  const screenshot = images.get(selected?.screenshots[frame] ?? '')
  return (
    <div className="grid gap-4">
      <div>
        <label htmlFor="desktop-task" className="block text-sm font-medium">
          Task
        </label>
        <Select
          value={task?.id ?? ''}
          onValueChange={(id) => {
            const index = report.tasks.findIndex((t) => t.id === id)
            if (index >= 0) {
              setTaskIndex(index)
              setAttemptIndex(0)
              setFrame(0)
            }
          }}
        >
          <SelectTrigger id="desktop-task" className="mt-1">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {report.tasks.map((t) => (
              <SelectItem key={t.id} value={t.id}>
                {t.id}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {task && <p className="mt-2">{task.prompt}</p>}
      </div>
      {rows.length ? (
        <div className="overflow-x-auto rounded-lg border">
          <Table className="w-full text-sm">
            <TableCaption className="sr-only">Desktop attempts for the selected task</TableCaption>
            <TableHeader>
              <TableRow>
                {['Harness', 'Model', 'Trial', 'Grade', 'Run outcome', 'Time', 'Router cost'].map(
                  (h) => (
                    <TableHead key={h} scope="col">
                      {h}
                    </TableHead>
                  ),
                )}
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((r, i) => (
                <TableRow key={r.id} className={i === attemptIndex ? 'bg-muted/60' : ''}>
                  <TableCell>
                    <Button
                      variant="link"
                      className="h-auto p-0"
                      type="button"
                      aria-pressed={i === attemptIndex}
                      aria-label={`Show ${r.harness}, ${r.model}, trial ${r.trial}`}
                      onClick={() => {
                        setAttemptIndex(i)
                        setFrame(0)
                      }}
                    >
                      {r.harness}
                    </Button>
                  </TableCell>
                  <TableCell>{r.model}</TableCell>
                  <TableCell>{r.trial}</TableCell>
                  <TableCell>{grade(r.resolved)}</TableCell>
                  <TableCell>{r.outcome.replaceAll('_', ' ')}</TableCell>
                  <TableCell>{r.seconds.toLocaleString()} s</TableCell>
                  <TableCell>
                    {r.router_usd === null
                      ? r.usage_status.replaceAll('_', ' ')
                      : fmtMoneyFine(r.router_usd)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      ) : (
        <p>No attempts for this task.</p>
      )}
      {selected && (
        <section aria-label="Attempt screenshots" className="grid justify-items-start gap-2">
          <h3 className="font-semibold">Screenshots</h3>
          {selected.screenshots.length ? (
            <>
              {screenshot ? (
                <Screenshot
                  key={selected.id + '-' + frame}
                  file={screenshot}
                  index={frame}
                  count={selected.screenshots.length}
                />
              ) : (
                <p className="text-sm text-muted-foreground">
                  Screenshot unavailable. Open the report folder to include its PNG files.
                </p>
              )}
              <div className="flex items-center gap-3 text-sm">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={frame === 0}
                  onClick={() => setFrame(frame - 1)}
                >
                  Previous
                </Button>
                {frame + 1} of {selected.screenshots.length}
                <Button
                  variant="outline"
                  size="sm"
                  disabled={frame + 1 >= selected.screenshots.length}
                  onClick={() => setFrame(frame + 1)}
                >
                  Next
                </Button>
              </div>
            </>
          ) : (
            <p className="text-sm text-muted-foreground">No screenshots recorded.</p>
          )}
        </section>
      )}
    </div>
  )
}

export function DesktopReport() {
  const [bundle, setBundle] = useState<Bundle & { revision: number }>()
  const [error, setError] = useState('')
  const generation = useRef(0)
  async function open(files: File[]) {
    if (!files.length) return
    const id = ++generation.current
    try {
      const result = await readBundle(files)
      if (generation.current === id) {
        setBundle({ ...result, revision: id })
        setError('')
      }
    } catch (e) {
      if (generation.current === id) {
        setBundle(undefined)
        setError(
          e instanceof SyntaxError
            ? 'The report contains invalid JSON.'
            : e instanceof Error
              ? e.message
              : 'Could not read this report.',
        )
      }
    }
  }
  return (
    <section className="grid gap-4 rounded-lg border p-5" aria-label="Desktop eval report">
      <div>
        <h2 className="text-lg font-semibold">Desktop eval report</h2>
        <p className="text-sm text-muted-foreground">
          Open a local report folder to inspect attempts and screenshots, or select the JSON file
          for results only.
        </p>
      </div>
      <div className="flex flex-wrap gap-4 text-sm">
        <label htmlFor="desktop-report-folder" className="grid gap-1 font-medium">
          Report folder
          <Input
            id="desktop-report-folder"
            type="file"
            multiple
            {...{ webkitdirectory: '' }}
            onChange={(e) => void open(Array.from(e.currentTarget.files ?? []))}
          />
        </label>
        <label htmlFor="desktop-report-json" className="grid gap-1 font-medium">
          Report JSON
          <Input
            id="desktop-report-json"
            type="file"
            accept=".json,application/json"
            onChange={(e) => void open(Array.from(e.currentTarget.files ?? []))}
          />
        </label>
      </div>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {bundle && <ReportDetails key={bundle.revision} {...bundle} />}
    </section>
  )
}
