/**
 * `/evals/$harness` (docs/superpowers/specs/2026-11-23-evals-design.md): tick sessions, generate a dataset (tasks +
 * models), pick from it, run, and compare the tasks across the picked models.
 *
 * ponytail: the dataset, the picks and the results live in page state, not the URL: no server stores datasets yet, and
 * mock state doesn't survive a reload. Put the dataset id in the URL once one does.
 */
import { Link } from '@tanstack/react-router'
import { Star } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { errorCode } from '@nasiko/data-access/detect'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { StateCard } from '@nasiko/ui/composites/state-card'
import { fmtInt, fmtLatency, fmtLocalDateTime, fmtMoneyFine } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { asButton, Button } from '@nasiko/ui/primitives/button'
import { Checkbox } from '@nasiko/ui/primitives/checkbox'
import { Label } from '@nasiko/ui/primitives/label'
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableFooter,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import { useEvalSessions, useGenerateDataset, useRunEvals } from './api'
import { copy } from './copy'
import { EvalsError } from './EvalsPage'
import { comparison, harnessName } from './logic'
import type { EvalDataset, EvalResult } from './types'

const key = (ids: readonly string[]) => [...ids].sort().join('\n')
const toggle = (ids: readonly string[], id: string, on: boolean) =>
  on ? [...ids, id] : ids.filter((x) => x !== id)

function Section({
  id,
  title,
  hint,
  children,
}: {
  id: string
  title: string
  hint?: string
  children: ReactNode
}) {
  return (
    <section
      aria-labelledby={id}
      className="flex flex-col gap-3 rounded-xl border border-border bg-card p-4"
    >
      <div>
        <h2 id={id} className="text-base font-semibold">
          {title}
        </h2>
        {hint ? <p className="text-sm text-muted-foreground">{hint}</p> : null}
      </div>
      {children}
    </section>
  )
}

function CheckRow({
  id,
  checked,
  onChange,
  children,
}: {
  id: string
  checked: boolean
  onChange: (on: boolean) => void
  children: ReactNode
}) {
  return (
    <li className="flex items-start gap-2">
      <Checkbox id={id} checked={checked} onCheckedChange={onChange} className="mt-0.5" />
      <Label htmlFor={id} className="font-normal">
        {children}
      </Label>
    </li>
  )
}

export function HarnessEvalsPage({ harness }: { harness: string }) {
  const name = harnessName(harness)
  const sessions = useEvalSessions(harness)
  const generate = useGenerateDataset()
  const run = useRunEvals()
  const [ticked, setTicked] = useState<string[]>([])
  const [picks, setPicks] = useState<{ tasks: string[]; models: string[] }>({
    tasks: [],
    models: [],
  })

  // A dataset belongs to the sessions it was built from: tick another set and it's gone until Regenerate.
  const dataset =
    generate.data && key(generate.variables?.session_ids ?? []) === key(ticked)
      ? generate.data
      : null
  const ran =
    dataset && run.data && run.variables?.datasetId === dataset.id
      ? { ...run.variables, results: run.data.results }
      : null
  const stale =
    !!ran && (key(ran.task_ids) !== key(picks.tasks) || key(ran.model_ids) !== key(picks.models))

  const onGenerate = () => {
    run.reset()
    generate.mutate(
      { harness, session_ids: ticked },
      {
        onSuccess: (d) =>
          setPicks({
            tasks: d.tasks.map((t) => t.id),
            models: d.models.filter((m) => m.used).map((m) => m.id),
          }),
      },
    )
  }

  const header = (
    <>
      <TopBar trail={[name]} />
      <PageHeader title={name} description={copy.description} />
    </>
  )

  if (sessions.isPending)
    return (
      <div className="mx-auto flex w-full max-w-page flex-col gap-4">
        {header}
        <PageLoader label={copy.loading} />
      </div>
    )
  if (sessions.isError)
    return (
      <div className="mx-auto flex w-full max-w-page flex-col gap-4">
        {header}
        {errorCode(sessions.error) === 'harness_not_connected' ? (
          <StateCard
            title={copy.notFound}
            action={
              <Link to="/evals" {...asButton({ size: 'sm', variant: 'outline' })}>
                {copy.back}
              </Link>
            }
          >
            {copy.notFoundBody}
          </StateCard>
        ) : (
          <EvalsError error={sessions.error} retry={() => void sessions.refetch()} />
        )}
      </div>
    )

  const rows = sessions.data
  const all = rows.length > 0 && ticked.length === rows.length
  return (
    <div className="mx-auto flex w-full max-w-page flex-col gap-4">
      {header}
      <Section
        id="evals-sessions"
        title={copy.sessions.title}
        hint={rows.length ? copy.sessions.hint : undefined}
      >
        {!rows.length ? (
          <p className="text-sm text-muted-foreground">{copy.sessions.none}</p>
        ) : (
          <>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-8">
                    <Checkbox
                      aria-label={copy.sessions.all}
                      checked={all}
                      indeterminate={!all && ticked.length > 0}
                      onCheckedChange={(on) => setTicked(on ? rows.map((s) => s.session_id) : [])}
                    />
                  </TableHead>
                  <TableHead>{copy.sessions.cols.title}</TableHead>
                  <TableHead>{copy.sessions.cols.started}</TableHead>
                  <TableHead className="text-right">{copy.sessions.cols.turns}</TableHead>
                  <TableHead className="text-right">{copy.sessions.cols.cost}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((s) => (
                  <TableRow key={s.session_id}>
                    <TableCell>
                      <Checkbox
                        aria-label={copy.sessions.one(s.title)}
                        checked={ticked.includes(s.session_id)}
                        onCheckedChange={(on) => setTicked((t) => toggle(t, s.session_id, on))}
                      />
                    </TableCell>
                    <TableCell className="max-w-0 min-w-56 truncate">{s.title}</TableCell>
                    <TableCell className="whitespace-nowrap text-muted-foreground">
                      {fmtLocalDateTime(s.started_at)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">{fmtInt(s.turns)}</TableCell>
                    <TableCell className="text-right tabular-nums">
                      {fmtMoneyFine(s.cost_usd)}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            <div className="flex flex-wrap items-center gap-3">
              <Button disabled={!ticked.length || generate.isPending} onClick={onGenerate}>
                {generate.isPending
                  ? copy.generating
                  : generate.data && !dataset
                    ? copy.regenerate(ticked.length)
                    : copy.generate(ticked.length)}
              </Button>
              {generate.isError ? (
                <p role="alert" className="text-sm text-destructive">
                  {copy.generateFailed}
                </p>
              ) : null}
            </div>
          </>
        )}
      </Section>

      {dataset ? (
        <DatasetSection
          dataset={dataset}
          picks={picks}
          setPicks={setPicks}
          running={run.isPending}
          failed={run.isError}
          onRun={() =>
            run.mutate({ datasetId: dataset.id, task_ids: picks.tasks, model_ids: picks.models })
          }
        />
      ) : null}

      {dataset && ran ? (
        <Results
          dataset={dataset}
          taskIds={ran.task_ids}
          modelIds={ran.model_ids}
          results={ran.results}
          stale={stale}
        />
      ) : null}
    </div>
  )
}

function DatasetSection({
  dataset,
  picks,
  setPicks,
  running,
  failed,
  onRun,
}: {
  dataset: EvalDataset
  picks: { tasks: string[]; models: string[] }
  setPicks: (
    f: (p: { tasks: string[]; models: string[] }) => { tasks: string[]; models: string[] },
  ) => void
  running: boolean
  failed: boolean
  onRun: () => void
}) {
  return (
    <Section id="evals-dataset" title={copy.dataset.title}>
      <div className="grid gap-6 md:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
        <div className="flex flex-col gap-2">
          <h3 className="text-sm font-medium">{copy.dataset.tasks}</h3>
          <p className="text-xs text-muted-foreground">{copy.dataset.tasksHint}</p>
          <ul className="flex flex-col gap-2">
            {dataset.tasks.map((t) => (
              <CheckRow
                key={t.id}
                id={`task-${t.id}`}
                checked={picks.tasks.includes(t.id)}
                onChange={(on) => setPicks((p) => ({ ...p, tasks: toggle(p.tasks, t.id, on) }))}
              >
                {t.title}
              </CheckRow>
            ))}
          </ul>
        </div>
        <div className="flex flex-col gap-2">
          <h3 className="text-sm font-medium">{copy.dataset.models}</h3>
          <p className="text-xs text-muted-foreground">{copy.dataset.modelsHint}</p>
          <ul className="flex flex-col gap-2">
            {dataset.models.map((m) => (
              <CheckRow
                key={m.id}
                id={`model-${m.id}`}
                checked={picks.models.includes(m.id)}
                onChange={(on) => setPicks((p) => ({ ...p, models: toggle(p.models, m.id, on) }))}
              >
                {m.name}
                {m.used ? (
                  <span className="ml-2 text-xs text-muted-foreground">{copy.dataset.used}</span>
                ) : null}
              </CheckRow>
            ))}
          </ul>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-3">
        <Button disabled={!picks.tasks.length || !picks.models.length || running} onClick={onRun}>
          {running ? copy.running : copy.run}
        </Button>
        <span className="text-sm text-muted-foreground">
          {copy.dataset.picked(picks.tasks.length, picks.models.length)}
        </span>
        {failed ? (
          <p role="alert" className="text-sm text-destructive">
            {copy.runFailed}
          </p>
        ) : null}
      </div>
    </Section>
  )
}

function Figures({
  score,
  cost,
  latency,
  best,
}: {
  score: number
  cost: number
  latency: number
  best?: boolean
}) {
  return (
    <div className="flex flex-col items-end gap-0.5">
      <span
        className={cn(
          'flex items-center gap-1 font-semibold tabular-nums',
          best && 'text-primary-text',
        )}
      >
        {best ? (
          <>
            <Star className="size-3.5 fill-current" aria-hidden />
            <span className="sr-only">{copy.results.best}: </span>
          </>
        ) : null}
        {copy.results.score(score)}
      </span>
      <span className="text-xs text-muted-foreground tabular-nums">
        {fmtMoneyFine(cost)} · {fmtLatency(latency)}
      </span>
    </div>
  )
}

function Results({
  dataset,
  taskIds,
  modelIds,
  results,
  stale,
}: {
  dataset: EvalDataset
  taskIds: string[]
  modelIds: string[]
  results: EvalResult[]
  stale: boolean
}) {
  const { rows, footer } = comparison(taskIds, modelIds, results)
  const taskTitle = new Map(dataset.tasks.map((t) => [t.id, t.title]))
  const modelName = new Map(dataset.models.map((m) => [m.id, m.name]))
  return (
    <Section id="evals-results" title={copy.results.title}>
      {stale ? <p className="text-sm text-muted-foreground">{copy.results.stale}</p> : null}
      <Table>
        <TableCaption>{copy.results.caption}</TableCaption>
        <TableHeader>
          <TableRow>
            <TableHead>{copy.results.task}</TableHead>
            {modelIds.map((m) => (
              <TableHead key={m} className="text-right">
                {modelName.get(m) ?? m}
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((row) => (
            <TableRow key={row.taskId}>
              <TableCell className="max-w-0 min-w-56 truncate">
                {taskTitle.get(row.taskId) ?? row.taskId}
              </TableCell>
              {row.cells.map((c, i) => (
                <TableCell key={modelIds[i]} className="text-right">
                  {c ? (
                    <Figures {...c} />
                  ) : (
                    <span className="text-muted-foreground">{copy.results.noResult}</span>
                  )}
                </TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
        <TableFooter>
          <TableRow>
            <TableCell className="font-medium">{copy.results.summary}</TableCell>
            {footer.map((f, i) => (
              <TableCell key={modelIds[i]} className="text-right">
                {f ? (
                  <Figures {...f} />
                ) : (
                  <span className="text-muted-foreground">{copy.results.noResult}</span>
                )}
              </TableCell>
            ))}
          </TableRow>
        </TableFooter>
      </Table>
    </Section>
  )
}
