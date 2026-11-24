/**
 * `/evals/$harness` (docs/superpowers/specs/2026-11-23-evals-design.md): tick sessions, generate a dataset (the server
 * links each session to its commit, then builds, tests and screens a task from it), pick verified tasks and models, run,
 * and compare the tasks across the models and the session that did the work.
 *
 * The dataset and the run are the URL's (`?dataset=`, `?run=`): both take minutes to hours, and are polled until they
 * end, so a person can leave and come back, or reopen a saved run from /evals.
 */
import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Star } from 'lucide-react'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { errorCode } from '@nasiko/data-access/detect'
import { useGithubConnect } from '@nasiko/features/deploy/github'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { StateCard } from '@nasiko/ui/composites/state-card'
import { fmtInt, fmtLatency, fmtLocalDateTime, fmtMoneyFine } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { Alert, AlertDescription, AlertTitle } from '@nasiko/ui/primitives/alert'
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
import {
  evalsKeys,
  useEvalDataset,
  useEvalRun,
  useEvalSessions,
  useEvalSetup,
  useGenerateDataset,
  useRunEvals,
} from './api'
import { copy } from './copy'
import { EvalsError } from './EvalsPage'
import { building, comparison, defaultPicks, harnessName, type Cell } from './logic'
import type { EvalsSearch } from './search'
import type { EvalDataset, EvalRun } from './types'

type Picks = { tasks: string[]; models: string[] }

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
        {/* Focusable from script only: a finished step moves focus here (`reveal`). */}
        <h2 id={id} tabIndex={-1} className="text-base font-semibold outline-none">
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
  disabled,
  onChange,
  children,
}: {
  id: string
  checked: boolean
  disabled?: boolean
  onChange: (on: boolean) => void
  children: ReactNode
}) {
  return (
    <li className="flex items-start gap-2">
      <Checkbox
        id={id}
        checked={checked}
        disabled={disabled}
        onCheckedChange={onChange}
        className="mt-0.5"
      />
      <Label htmlFor={id} className={cn('font-normal', disabled && 'text-muted-foreground')}>
        {children}
      </Label>
    </li>
  )
}

/** Connect GitHub when it isn't: the page's evals read sessions' repositories through it. */
function GithubBanner() {
  const qc = useQueryClient()
  const { state, connect } = useGithubConnect()
  // The connection is made in a popup: once the wait for it ends, the sessions' eligibility may have changed.
  const waited = useRef(false)
  useEffect(() => {
    if (state.kind === 'waiting') waited.current = true
    else if (waited.current) {
      waited.current = false
      void qc.invalidateQueries({ queryKey: evalsKeys.all })
    }
  }, [state.kind, qc])
  return (
    <Alert>
      <AlertTitle>{copy.github.title}</AlertTitle>
      <AlertDescription className="flex flex-col items-start gap-2">
        <span>{copy.github.body}</span>
        {state.kind === 'blocked' ? (
          <span>
            {copy.github.blocked}{' '}
            <a href={state.url} target="_blank" rel="noreferrer" className="underline">
              {copy.github.openIt}
            </a>
          </span>
        ) : null}
        {state.kind === 'failed' ? <span role="alert">{copy.github.failed}</span> : null}
        <Button size="sm" disabled={state.kind === 'waiting'} onClick={() => void connect()}>
          {state.kind === 'waiting' ? copy.github.waiting : copy.github.action}
        </Button>
      </AlertDescription>
    </Alert>
  )
}

export function HarnessEvalsPage({
  harness,
  search,
  setSearch,
}: {
  harness: string
  search: EvalsSearch
  setSearch: (patch: Partial<EvalsSearch>) => void
}) {
  const name = harnessName(harness)
  const sessions = useEvalSessions(harness)
  const setup = useEvalSetup()
  const generate = useGenerateDataset()
  const startRun = useRunEvals()
  const dataset = useEvalDataset(search.dataset)
  const run = useEvalRun(search.run)
  const [ticked, setTicked] = useState<string[]>([])
  // Picks belong to one dataset: another dataset starts from its own defaults.
  const [picked, setPicked] = useState<(Picks & { for: string }) | null>(null)
  const ds = dataset.data
  const picks: Picks | null = ds ? (picked?.for === ds.id ? picked : defaultPicks(ds)) : null
  const setPicks = (f: (p: Picks) => Picks) => {
    if (ds && picks) setPicked({ for: ds.id, ...f(picks) })
  }

  // A finished Generate or Run shows its section below a long list, out of sight of the button that made it, so the
  // section's heading takes the view and focus (as /optimization#settings does). A new object per success, so a
  // second run moves there again; nothing else moves the page.
  // The section appears once the URL names its dataset or run, a render after the success: wait for it, once.
  const [reveal, setReveal] = useState<{ id: string } | null>(null)
  const revealed = useRef<{ id: string } | null>(null)
  useEffect(() => {
    if (!reveal || revealed.current === reveal) return
    const h = document.getElementById(reveal.id)
    if (!h) return
    revealed.current = reveal
    h.scrollIntoView({ block: 'start' })
    h.focus({ preventScroll: true })
  }, [reveal, search.dataset, search.run])

  const onGenerate = () => {
    startRun.reset()
    generate.mutate(
      { harness, session_ids: ticked },
      {
        onSuccess: (d) => {
          setSearch({ dataset: d.id, run: undefined })
          setReveal({ id: 'evals-dataset' })
        },
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
  const usable = rows.filter((s) => s.eligible).map((s) => s.session_id)
  const all = usable.length > 0 && usable.every((id) => ticked.includes(id))
  const canBuild = !setup.data || (setup.data.runner_available && setup.data.chat_history)
  return (
    <div className="mx-auto flex w-full max-w-page flex-col gap-4">
      {header}
      {setup.data && !setup.data.github_connected ? <GithubBanner /> : null}
      {setup.data && !setup.data.runner_available ? (
        <p className="text-sm text-muted-foreground">{copy.notRunning}</p>
      ) : setup.data && !setup.data.chat_history ? (
        <p className="text-sm text-muted-foreground">{copy.noChatHistory}</p>
      ) : null}
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
                      disabled={!usable.length}
                      indeterminate={!all && ticked.length > 0}
                      onCheckedChange={(on) => setTicked(on ? usable : [])}
                    />
                  </TableHead>
                  <TableHead>{copy.sessions.cols.title}</TableHead>
                  <TableHead>{copy.sessions.cols.repo}</TableHead>
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
                        disabled={!s.eligible}
                        onCheckedChange={(on) => setTicked((t) => toggle(t, s.session_id, on))}
                      />
                    </TableCell>
                    <TableCell className="max-w-0 min-w-56">
                      <span className="block truncate">{s.title}</span>
                      {!s.eligible && s.ineligible_reason ? (
                        <span className="block text-xs text-muted-foreground">
                          {copy.sessions.ineligible[s.ineligible_reason] ?? s.ineligible_reason}
                        </span>
                      ) : null}
                    </TableCell>
                    <TableCell className="whitespace-nowrap text-muted-foreground">
                      {s.repo ?? '–'}
                    </TableCell>
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
              <Button
                disabled={!ticked.length || generate.isPending || !canBuild}
                onClick={onGenerate}
              >
                {generate.isPending
                  ? copy.generating
                  : search.dataset
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

      {search.dataset ? (
        <DatasetSection
          query={dataset}
          picks={picks}
          setPicks={setPicks}
          starting={startRun.isPending}
          failed={startRun.isError}
          onRun={() => {
            if (!ds || !picks) return
            startRun.mutate(
              { datasetId: ds.id, task_ids: picks.tasks, model_ids: picks.models },
              {
                onSuccess: (r) => {
                  setSearch({ run: r.id })
                  setReveal({ id: 'evals-results' })
                },
              },
            )
          }}
        />
      ) : null}

      {ds && search.run ? <Results dataset={ds} query={run} picks={picks} /> : null}
    </div>
  )
}

function DatasetSection({
  query,
  picks,
  setPicks,
  starting,
  failed,
  onRun,
}: {
  query: ReturnType<typeof useEvalDataset>
  picks: Picks | null
  setPicks: (f: (p: Picks) => Picks) => void
  starting: boolean
  failed: boolean
  onRun: () => void
}) {
  if (query.isPending)
    return (
      <Section id="evals-dataset" title={copy.dataset.title}>
        <PageLoader label={copy.loading} />
      </Section>
    )
  if (query.isError)
    return (
      <Section id="evals-dataset" title={copy.dataset.title}>
        <StateCard
          tone="error"
          title={copy.dataset.loadFailed}
          action={
            <Button size="sm" variant="outline" onClick={() => void query.refetch()}>
              {copy.retry}
            </Button>
          }
        />
      </Section>
    )
  const dataset = query.data
  if (building(dataset))
    return (
      <Section id="evals-dataset" title={copy.dataset.title}>
        <p role="status" className="text-sm text-muted-foreground">
          {copy.dataset.building(dataset.progress.done, dataset.progress.total)}
        </p>
      </Section>
    )
  if (dataset.status === 'failed')
    return (
      <Section id="evals-dataset" title={copy.dataset.title}>
        <p role="alert" className="text-sm text-destructive">
          {copy.dataset.failed(dataset.error)}
        </p>
      </Section>
    )
  const sessions = Object.keys(dataset.outcomes).length
  const tasks = Object.values(dataset.outcomes).filter((o) => o === 'task').length
  return (
    <Section
      id="evals-dataset"
      title={copy.dataset.title}
      hint={sessions ? copy.dataset.outcomes(tasks, sessions) : undefined}
    >
      {!dataset.tasks.length ? (
        <p className="text-sm text-muted-foreground">{copy.dataset.noTasks}</p>
      ) : (
        <DatasetPicks
          dataset={dataset}
          picks={picks ?? { tasks: [], models: [] }}
          setPicks={setPicks}
          starting={starting}
          failed={failed}
          onRun={onRun}
        />
      )}
    </Section>
  )
}

function DatasetPicks({
  dataset,
  picks,
  setPicks,
  starting,
  failed,
  onRun,
}: {
  dataset: EvalDataset
  picks: Picks
  setPicks: (f: (p: Picks) => Picks) => void
  starting: boolean
  failed: boolean
  onRun: () => void
}) {
  return (
    <>
      <div className="grid gap-6 md:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
        <div className="flex flex-col gap-2">
          <h3 className="text-sm font-medium">{copy.dataset.tasks}</h3>
          <p className="text-xs text-muted-foreground">{copy.dataset.tasksHint}</p>
          <ul className="flex flex-col gap-2">
            {dataset.tasks.map((t) => {
              const runnable = t.status === 'verified'
              return (
                <CheckRow
                  key={t.id}
                  id={`task-${t.id}`}
                  checked={runnable && picks.tasks.includes(t.id)}
                  disabled={!runnable}
                  onChange={(on) => setPicks((p) => ({ ...p, tasks: toggle(p.tasks, t.id, on) }))}
                >
                  {t.title}
                  <span className="ml-2 text-xs text-muted-foreground">
                    {t.repo} · {copy.dataset.status[t.status] ?? t.status}
                    {t.reject_reason ? ` · ${t.reject_reason}` : ''}
                  </span>
                </CheckRow>
              )
            })}
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
        <Button disabled={!picks.tasks.length || !picks.models.length || starting} onClick={onRun}>
          {starting ? copy.running : copy.run}
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
    </>
  )
}

function Figures({
  score,
  cost,
  latency,
  best,
  builder,
}: {
  score: number
  cost: number | null
  latency: number | null
  best?: boolean
  builder?: boolean
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
        {cost == null ? '–' : fmtMoneyFine(cost)} · {latency == null ? '–' : fmtLatency(latency)}
      </span>
      {builder ? (
        <span className="text-xs text-muted-foreground">{copy.results.optimistic}</span>
      ) : null}
    </div>
  )
}

function CellView({ cell }: { cell: Cell | null }) {
  if (cell?.kind === 'graded') return <Figures {...cell} />
  return (
    <span className="text-muted-foreground">
      {cell?.kind === 'waiting' ? copy.results.waiting : copy.results.noResult}
    </span>
  )
}

function Results({
  dataset,
  query,
  picks,
}: {
  dataset: EvalDataset
  query: ReturnType<typeof useEvalRun>
  picks: Picks | null
}) {
  if (query.isPending)
    return (
      <Section id="evals-results" title={copy.results.title}>
        <PageLoader label={copy.loading} />
      </Section>
    )
  if (query.isError)
    return (
      <Section id="evals-results" title={copy.results.title}>
        <StateCard
          tone="error"
          title={copy.results.loadFailed}
          action={
            <Button size="sm" variant="outline" onClick={() => void query.refetch()}>
              {copy.retry}
            </Button>
          }
        />
      </Section>
    )
  return <ResultsTable dataset={dataset} run={query.data} picks={picks} />
}

function ResultsTable({
  dataset,
  run,
  picks,
}: {
  dataset: EvalDataset
  run: EvalRun
  picks: Picks | null
}) {
  const ran = new Set(run.results.map((r) => r.task_id))
  const tasks = dataset.tasks.filter((t) => ran.has(t.id))
  const taskIds = tasks.map((t) => t.id)
  const modelIds = run.models.map((m) => m.id)
  const { rows, footer } = comparison(taskIds, modelIds, run.results)
  const task = new Map(tasks.map((t) => [t.id, t]))
  const stale =
    !!picks && (key(taskIds) !== key(picks.tasks) || key(modelIds) !== key(picks.models))
  const going = run.status === 'queued' || run.status === 'running' || run.status === 'finalizing'
  return (
    <Section id="evals-results" title={copy.results.title}>
      {going ? (
        <p role="status" className="text-sm text-muted-foreground">
          {copy.results.progress(run.progress.done, run.progress.total)}
        </p>
      ) : run.status === 'failed' || run.status === 'cancelled' ? (
        <p role="alert" className="text-sm text-destructive">
          {copy.results.ended(run.status)}
        </p>
      ) : null}
      {stale ? <p className="text-sm text-muted-foreground">{copy.results.stale}</p> : null}
      <Table>
        <TableCaption>{copy.results.caption}</TableCaption>
        <TableHeader>
          <TableRow>
            <TableHead>{copy.results.task}</TableHead>
            <TableHead className="text-right">{copy.results.original}</TableHead>
            {run.models.map((m) => (
              <TableHead key={m.id} className="text-right">
                {m.name}
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((row) => {
            const t = task.get(row.taskId)
            return (
              <TableRow key={row.taskId}>
                <TableCell className="max-w-0 min-w-56 truncate">
                  {t?.title ?? row.taskId}
                </TableCell>
                <TableCell className="text-right">
                  {t ? (
                    <div className="flex flex-col items-end gap-0.5">
                      <span className="text-sm">{t.original.model}</span>
                      <span className="text-xs text-muted-foreground tabular-nums">
                        {t.original.cost_usd == null ? '–' : fmtMoneyFine(t.original.cost_usd)} ·{' '}
                        {t.original.latency_ms == null ? '–' : fmtLatency(t.original.latency_ms)}
                      </span>
                    </div>
                  ) : null}
                </TableCell>
                {row.cells.map((c, i) => (
                  <TableCell key={modelIds[i]} className="text-right">
                    <CellView cell={c} />
                  </TableCell>
                ))}
              </TableRow>
            )
          })}
        </TableBody>
        <TableFooter>
          <TableRow>
            <TableCell className="font-medium">{copy.results.summary}</TableCell>
            <TableCell />
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
