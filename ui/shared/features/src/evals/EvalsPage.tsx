/**
 * `/evals` (docs/superpowers/specs/2026-11-23-evals-design.md): Claude Code's evals, as a stepper. The first step is
 * the dataset's source: tick sessions (the server links each to its commit, then builds, tests and screens a task from
 * it), or, when Claude Code has none, pick pre-defined SWE benchmarks. Then pick verified tasks and models, run, and
 * compare the tasks across the models and the session that did the work. A step opens once what it shows exists. Not
 * connected: the way to connect it.
 *
 * The dataset and the run are the URL's (`?dataset=`, `?run=`): both take minutes to hours and are polled until they
 * end, so a person can leave and come back, or reopen a saved run from the list under the sessions.
 */
import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ChevronLeft, ChevronRight, FlaskConical, Star } from 'lucide-react'
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { errorCode, isEndpointAbsent } from '@nasiko/data-access/detect'
import { useGithubConnect } from '@nasiko/features/deploy/github'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { EmptyState, StateCard } from '@nasiko/ui/composites/state-card'
import { Stepper } from '@nasiko/ui/composites/stepper'
import {
  fmtInt,
  fmtLatency,
  fmtLocalDateTime,
  fmtMoneyFine,
  fmtShortDay,
} from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { Alert, AlertDescription, AlertTitle } from '@nasiko/ui/primitives/alert'
import { Badge } from '@nasiko/ui/primitives/badge'
import { asButton, Button } from '@nasiko/ui/primitives/button'
import { Checkbox } from '@nasiko/ui/primitives/checkbox'
import { Label } from '@nasiko/ui/primitives/label'
import { Progress } from '@nasiko/ui/primitives/progress'
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
  useEvalBenchmarks,
  useEvalDataset,
  useEvalRun,
  useEvalRuns,
  useEvalSessions,
  useEvalSetup,
  useGenerateDataset,
  useReviewTask,
  useRunEvals,
} from './api'
import { copy } from './copy'
import { DesktopReport } from './DesktopReport'
import {
  building,
  canApprove,
  comparison,
  defaultPicks,
  needsReview,
  type Cell,
  type ModelTotals,
} from './logic'
import type { EvalsSearch } from './search'
import { evalSteps, STEP, type Source, type Step } from './steps'
import type { EvalBenchmark, EvalDataset, EvalRun, EvalSession } from './types'

/** ponytail: Claude Code is the one harness evals support; a harness step comes back with a second one. */
const HARNESS = 'claude'

interface Picks {
  tasks: string[]
  models: string[]
}

const key = (ids: readonly string[]) => [...ids].sort().join('\n')
const toggle = (ids: readonly string[], id: string, on: boolean) =>
  on ? [...ids, id] : ids.filter((x) => x !== id)

/** The step the URL points at: the run's comparison, the dataset's picks, or the source. */
const stepOf = (s: EvalsSearch): Step =>
  s.run ? STEP.compare : s.dataset ? STEP.dataset : STEP.source

// The table sits on the page (the stepper has no card), so its sticky head and the fade below match the page.
const STICKY_HEAD = 'sticky top-0 z-10 bg-background shadow-[inset_0_-1px_0_var(--border)]'

/** Whether a scroller has more past its start and past its end on one axis, kept up to date on scroll and resize. */
function useOverflow<T extends HTMLElement>(axis: 'x' | 'y') {
  const ref = useRef<T>(null)
  const [before, setBefore] = useState(false)
  const [after, setAfter] = useState(false)
  const measure = useCallback(() => {
    const el = ref.current
    if (!el) return
    const [at, seen, all] =
      axis === 'x'
        ? [el.scrollLeft, el.clientWidth, el.scrollWidth]
        : [el.scrollTop, el.clientHeight, el.scrollHeight]
    setBefore(at > 1)
    setAfter(at + seen < all - 1)
  }, [axis])
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const watch = new ResizeObserver(measure)
    watch.observe(el)
    return () => watch.disconnect()
  }, [measure])
  return { ref, before, after, measure }
}

/**
 * A table in a capped box that scrolls, so a step's footer stays on screen beside a long list. While rows run past its
 * bottom they fade out there: the cap lands wherever it lands in a row, and a few pixels of the next row under a row's
 * own border read as a second border (the user, 2026-11-24). shadcn's table container scrolls on its own and would
 * become the sticky head's scroller, so it stays visible and this box scrolls (ChartTable's pattern).
 */
function ScrollTable({ children }: { children: ReactNode }) {
  const { ref, before, after, measure } = useOverflow<HTMLDivElement>('y')
  return (
    <div className="relative">
      <div
        ref={ref}
        onScroll={measure}
        className="max-h-[26rem] overflow-auto rounded-lg border border-border [&>[data-slot=table-container]]:overflow-visible"
      >
        {children}
      </div>
      {/* Under the sticky head (the box's 1px border + the head's h-10), the same for rows scrolled up past it. */}
      <div
        aria-hidden
        className={cn(
          'pointer-events-none absolute inset-x-px top-[calc(2.5rem+1px)] h-8 bg-linear-to-b from-background transition-opacity duration-200',
          before ? 'opacity-100' : 'opacity-0',
        )}
      />
      <div
        aria-hidden
        className={cn(
          'pointer-events-none absolute inset-x-px bottom-px h-12 rounded-b-lg bg-linear-to-t from-background transition-opacity duration-200',
          after ? 'opacity-100' : 'opacity-0',
        )}
      />
    </div>
  )
}

/** What a failed evals read means: a server without the routes, or a failure worth retrying. */
function EvalsError({ error, retry }: { error: unknown; retry: () => void }) {
  return isEndpointAbsent(error) ? (
    <StateCard title={copy.absent}>{copy.absentBody}</StateCard>
  ) : (
    <StateCard
      tone="error"
      title={copy.failed}
      action={
        <Button size="sm" variant="outline" onClick={retry}>
          {copy.retry}
        </Button>
      }
    />
  )
}

/** A failed read of a step's own object (the dataset, the run), with a retry. */
function LoadFailed({ title, retry }: { title: string; retry: () => void }) {
  return (
    <StateCard
      tone="error"
      title={title}
      action={
        <Button size="sm" variant="outline" onClick={retry}>
          {copy.retry}
        </Button>
      }
    />
  )
}

/** A step's own heading and one line on what to do there. */
function StepHeading({ title, hint }: { title: string; hint?: string | undefined }) {
  return (
    <div className="mb-4">
      <h2 className="text-lg font-semibold">{title}</h2>
      {hint ? <p className="text-sm text-muted-foreground">{hint}</p> : null}
    </div>
  )
}

const Failed = ({ children }: { children: ReactNode }) => (
  <p role="alert" className="mt-3 text-sm text-destructive">
    {children}
  </p>
)

const Status = ({ children }: { children: ReactNode }) => (
  <p role="status" className="mb-3 text-sm text-muted-foreground">
    {children}
  </p>
)

function CheckRow({
  id,
  checked,
  disabled,
  onChange,
  children,
  actions,
}: {
  id: string
  checked: boolean
  disabled?: boolean
  onChange: (on: boolean) => void
  children: ReactNode
  /** Buttons after the label, outside it, so pressing one never toggles the box. */
  actions?: ReactNode
}) {
  return (
    <li className="flex min-h-9 items-center gap-2.5 rounded-md px-2 py-1 hover:bg-muted/50">
      <Checkbox id={id} checked={checked} disabled={disabled} onCheckedChange={onChange} />
      <Label
        htmlFor={id}
        className={cn(
          'flex min-w-0 flex-1 items-center gap-2 font-normal',
          disabled && 'text-muted-foreground',
        )}
      >
        {children}
      </Label>
      {actions}
    </li>
  )
}

/** Connect GitHub when it isn't: evals read the sessions' repositories through it. */
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
    <Alert className="mb-4">
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

export function EvalsPage({
  search,
  setSearch,
}: {
  search: EvalsSearch
  setSearch: (patch: Partial<EvalsSearch>) => void
}) {
  const [desktopOpen, setDesktopOpen] = useState(false)
  const sessions = useEvalSessions(HARNESS)
  const setup = useEvalSetup()
  const benchmarks = useEvalBenchmarks(sessions.isSuccess && !sessions.data.length)
  const generate = useGenerateDataset()
  const startRun = useRunEvals()
  const datasetQuery = useEvalDataset(search.dataset)
  const runQuery = useEvalRun(search.run)
  // The ticked sessions, or the picked benchmarks: whichever the first step offers.
  const [ticked, setTicked] = useState<string[]>([])
  // Picks belong to one dataset: another dataset starts from its own defaults.
  const [picked, setPicked] = useState<(Picks & { for: string }) | null>(null)

  // The step asked for: the URL's, until the stepper opens another. A new dataset or run in the URL (Generate, Run, a
  // saved run, Back) moves to its step.
  const [step, setStep] = useState<Step>(() => stepOf(search))
  const [seen, setSeen] = useState({ dataset: search.dataset, run: search.run })
  if (seen.dataset !== search.dataset || seen.run !== search.run) {
    setSeen({ dataset: search.dataset, run: search.run })
    setStep(stepOf(search))
  }

  const ds = datasetQuery.data
  const picks: Picks | null = ds ? (picked?.for === ds.id ? picked : defaultPicks(ds)) : null
  const setPicks = (f: (p: Picks) => Picks) => {
    if (ds && picks) setPicked({ for: ds.id, ...f(picks) })
  }

  // Narrower than `max-w-page`: a step's tables and cards read better a little tighter (the user, 2026-11-24).
  const frame = (children: ReactNode) => (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-4">
      <TopBar />
      <PageHeader
        title={copy.title}
        description={copy.description}
        actions={
          <Button
            variant="outline"
            size="sm"
            aria-expanded={desktopOpen}
            onClick={() => setDesktopOpen(!desktopOpen)}
          >
            {desktopOpen ? 'Back to replay evals' : 'Open desktop report'}
          </Button>
        }
      />
      {desktopOpen ? <DesktopReport /> : children}
    </div>
  )
  if (sessions.isPending) return frame(<PageLoader label={copy.loading} />)
  if (sessions.isError)
    return frame(
      errorCode(sessions.error) === 'harness_not_connected' ? (
        <EmptyState
          icon={FlaskConical}
          title={copy.empty.title}
          action={
            <Link to="/harnesses" {...asButton({ size: 'sm' })}>
              {copy.empty.action}
            </Link>
          }
        >
          {copy.empty.body}
        </EmptyState>
      ) : (
        <EvalsError error={sessions.error} retry={() => void sessions.refetch()} />
      ),
    )

  const source: Source = sessions.data.length ? 'sessions' : 'benchmarks'
  const fromBenchmarks = !!ds?.tasks.some((t) => t.benchmark_id)
  // The step asked for, unless what it shows isn't in the URL: then the last step that is.
  const shown: Step =
    step === STEP.compare && search.run
      ? STEP.compare
      : step !== STEP.source && search.dataset
        ? STEP.dataset
        : STEP.source
  const canOpen = (i: number) =>
    i === STEP.source ||
    (i === STEP.dataset && !!search.dataset) ||
    (i === STEP.compare && !!search.run)
  // Sessions need a server that runs evals and keeps chat history; unknown yet, the server decides.
  const canBuild = !setup.data || (setup.data.runner_available && setup.data.chat_history)

  const onGenerate = () => {
    startRun.reset()
    generate.mutate(
      source === 'sessions'
        ? { harness: HARNESS, session_ids: ticked }
        : { harness: HARNESS, benchmark_ids: ticked },
      { onSuccess: (d) => setSearch({ dataset: d.id, run: undefined }) },
    )
  }
  const onRun = () => {
    if (!ds || !picks) return
    startRun.mutate(
      { datasetId: ds.id, task_ids: picks.tasks, model_ids: picks.models },
      { onSuccess: (r) => setSearch({ run: r.id }) },
    )
  }

  const action =
    shown === STEP.source ? (
      <Button
        disabled={!ticked.length || generate.isPending || (source === 'sessions' && !canBuild)}
        onClick={onGenerate}
      >
        {source === 'benchmarks'
          ? generate.isPending
            ? copy.opening
            : copy.goToDataset
          : generate.isPending
            ? copy.generating
            : search.dataset
              ? copy.regenerate(ticked.length)
              : copy.generate(ticked.length)}
      </Button>
    ) : shown === STEP.dataset ? (
      <Button
        disabled={
          !ds ||
          ds.status !== 'ready' ||
          !picks?.tasks.length ||
          !picks.models.length ||
          startRun.isPending
        }
        onClick={onRun}
      >
        {startRun.isPending ? copy.running : copy.run}
      </Button>
    ) : undefined

  return frame(
    <Stepper
      label={copy.steps.label}
      // Room for the current step's pulse, which grows past its circle, under the page header.
      className="mt-6"
      steps={evalSteps(source, {
        source: ticked.length
          ? source === 'sessions'
            ? copy.sessions.ticked(ticked.length)
            : copy.benchmarks.picked(ticked.length)
          : undefined,
        dataset: picks ? copy.dataset.picked(picks.tasks.length, picks.models.length) : undefined,
      })}
      active={shown}
      canOpen={canOpen}
      onStepChange={(i) => setStep(i as Step)}
      action={action}
    >
      {shown === STEP.source ? (
        source === 'sessions' ? (
          <>
            <StepHeading title={copy.sessions.title} hint={copy.sessions.hint} />
            {setup.data && !setup.data.github_connected ? <GithubBanner /> : null}
            {setup.data && !setup.data.runner_available ? (
              <Status>{copy.notRunning}</Status>
            ) : setup.data && !setup.data.chat_history ? (
              <Status>{copy.noChatHistory}</Status>
            ) : null}
            <SessionsTable rows={sessions.data} ticked={ticked} setTicked={setTicked} />
            {generate.isError ? <Failed>{copy.generateFailed}</Failed> : null}
            <SavedEvals />
          </>
        ) : (
          <>
            <StepHeading title={copy.benchmarks.title} hint={copy.benchmarks.hint} />
            {benchmarks.isPending ? (
              <PageLoader label={copy.loading} />
            ) : benchmarks.isError ? (
              <EvalsError error={benchmarks.error} retry={() => void benchmarks.refetch()} />
            ) : !benchmarks.data.length ? (
              <p className="text-sm text-muted-foreground">{copy.benchmarks.none}</p>
            ) : (
              <BenchmarkPicks list={benchmarks.data} ticked={ticked} setTicked={setTicked} />
            )}
            {generate.isError ? <Failed>{copy.generateFailed}</Failed> : null}
            <SavedEvals />
          </>
        )
      ) : shown === STEP.dataset ? (
        <>
          <StepHeading title={copy.dataset.title} hint={copy.dataset.hint} />
          {datasetQuery.isPending ? (
            <PageLoader label={copy.loading} />
          ) : datasetQuery.isError ? (
            <LoadFailed title={copy.dataset.loadFailed} retry={() => void datasetQuery.refetch()} />
          ) : (
            <DatasetStep
              dataset={datasetQuery.data}
              fromBenchmarks={fromBenchmarks}
              picks={picks ?? { tasks: [], models: [] }}
              setPicks={setPicks}
            />
          )}
          {startRun.isError ? <Failed>{copy.runFailed}</Failed> : null}
        </>
      ) : (
        <>
          <StepHeading title={copy.results.title} />
          {runQuery.isPending || datasetQuery.isPending ? (
            <PageLoader label={copy.loading} />
          ) : runQuery.isError || datasetQuery.isError ? (
            <LoadFailed
              title={copy.results.loadFailed}
              retry={() => {
                void runQuery.refetch()
                void datasetQuery.refetch()
              }}
            />
          ) : (
            <Results dataset={datasetQuery.data} run={runQuery.data} picks={picks} />
          )}
        </>
      )}
    </Stepper>,
  )
}

function SessionsTable({
  rows,
  ticked,
  setTicked,
}: {
  rows: EvalSession[]
  ticked: string[]
  setTicked: (f: (t: string[]) => string[]) => void
}) {
  const usable = rows.filter((s) => s.eligible).map((s) => s.session_id)
  const all = usable.length > 0 && usable.every((id) => ticked.includes(id))
  return (
    <ScrollTable>
      <Table>
        <TableHeader className={STICKY_HEAD}>
          <TableRow>
            <TableHead className="w-10">
              <Checkbox
                aria-label={copy.sessions.all}
                checked={all}
                disabled={!usable.length}
                onCheckedChange={(on) => setTicked(() => (on ? usable : []))}
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
          {rows.map((s) => {
            const on = ticked.includes(s.session_id)
            return (
              <TableRow key={s.session_id} data-state={on ? 'selected' : undefined}>
                <TableCell>
                  <Checkbox
                    aria-label={copy.sessions.one(s.title)}
                    checked={on}
                    disabled={!s.eligible}
                    onCheckedChange={(v) => setTicked((t) => toggle(t, s.session_id, v))}
                  />
                </TableCell>
                <TableCell className="max-w-0 min-w-56">
                  <span className="block truncate font-medium">{s.title}</span>
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
            )
          })}
        </TableBody>
      </Table>
    </ScrollTable>
  )
}

/** One card per benchmark, several may be picked: their tasks together make the dataset's. */
function BenchmarkPicks({
  list,
  ticked,
  setTicked,
}: {
  list: EvalBenchmark[]
  ticked: string[]
  setTicked: (f: (t: string[]) => string[]) => void
}) {
  return (
    <ul className="grid gap-3 sm:grid-cols-2">
      {list.map((b) => {
        const on = ticked.includes(b.id)
        const id = `benchmark-${b.id}`
        return (
          <li key={b.id}>
            <Label
              htmlFor={id}
              className={cn(
                'flex h-full cursor-pointer items-start gap-3 rounded-lg border p-4 font-normal transition-[border-color,background-color] duration-200',
                on ? 'border-primary/50 bg-primary/5' : 'border-border hover:bg-muted/40',
              )}
            >
              <Checkbox
                id={id}
                aria-label={b.name}
                checked={on}
                onCheckedChange={(v) => setTicked((t) => toggle(t, b.id, v))}
                className="mt-0.5"
              />
              <span className="flex min-w-0 flex-1 flex-col gap-1">
                <span className="flex items-center justify-between gap-2">
                  <span className="truncate font-medium">{b.name}</span>
                  <Badge variant="secondary">{copy.benchmarks.tasks(b.tasks)}</Badge>
                </span>
                <span className="text-sm text-muted-foreground">{b.description}</span>
              </span>
            </Label>
          </li>
        )
      })}
    </ul>
  )
}

/** The viewer's runs, each reopening its comparison. */
function SavedEvals() {
  const q = useEvalRuns()
  if (q.isPending || q.isError || !q.data.length) return null
  return (
    <section aria-labelledby="evals-saved" className="mt-6 flex flex-col gap-2">
      <h3 id="evals-saved" className="text-sm font-medium">
        {copy.saved.title}
      </h3>
      <ul className="flex flex-col divide-y divide-border rounded-lg border border-border">
        {q.data.map((r) => {
          const models = r.models.map((m) => m.name).join(', ')
          return (
            <li key={r.id}>
              <Link
                to="/evals"
                search={{ dataset: r.dataset_id, run: r.id }}
                aria-label={copy.saved.open(models)}
                className="flex items-center justify-between gap-3 px-4 py-3 text-sm hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              >
                <span className="min-w-0 truncate">{models}</span>
                <span className="shrink-0 text-muted-foreground">
                  {copy.saved.status[r.status] ?? r.status} · {fmtShortDay(r.created_at)}
                </span>
              </Link>
            </li>
          )
        })}
      </ul>
    </section>
  )
}

function PickBox({ title, hint, children }: { title: string; hint: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-2 rounded-lg border border-border p-3">
      <div className="px-2">
        <h3 className="text-sm font-medium">{title}</h3>
        <p className="text-xs text-muted-foreground">{hint}</p>
      </div>
      {children}
    </div>
  )
}

/** The dataset while it is built, why it failed, or its tasks and models to pick from. */
function DatasetStep({
  dataset,
  fromBenchmarks,
  picks,
  setPicks,
}: {
  dataset: EvalDataset
  fromBenchmarks: boolean
  picks: Picks
  setPicks: (f: (p: Picks) => Picks) => void
}) {
  const review = useReviewTask(dataset.id)
  if (building(dataset))
    return <Status>{copy.dataset.building(dataset.progress.done, dataset.progress.total)}</Status>
  if (dataset.status === 'failed') return <Failed>{copy.dataset.failed(dataset.error)}</Failed>
  const sessions = Object.keys(dataset.outcomes).length
  const tasks = Object.values(dataset.outcomes).filter((o) => o === 'task').length
  if (!dataset.tasks.length)
    return <p className="text-sm text-muted-foreground">{copy.dataset.noTasks}</p>
  return (
    <>
      {sessions ? <Status>{copy.dataset.outcomes(tasks, sessions)}</Status> : null}
      <div className="grid gap-4 md:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        <PickBox
          title={copy.dataset.tasks}
          hint={fromBenchmarks ? copy.dataset.benchmarkTasksHint : copy.dataset.tasksHint}
        >
          <ul className="flex max-h-80 flex-col overflow-y-auto">
            {dataset.tasks.map((t) => {
              const runnable = t.status === 'verified'
              const reviewing = review.isPending && review.variables.taskId === t.id
              return (
                <CheckRow
                  key={t.id}
                  id={`task-${t.id}`}
                  checked={runnable && picks.tasks.includes(t.id)}
                  disabled={!runnable}
                  onChange={(on) => setPicks((p) => ({ ...p, tasks: toggle(p.tasks, t.id, on) }))}
                  actions={
                    canApprove(t) ? (
                      <span className="flex shrink-0 gap-1">
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={reviewing}
                          aria-label={copy.dataset.approve(t.title)}
                          onClick={() => review.mutate({ taskId: t.id, approve: true })}
                        >
                          {copy.dataset.approveShort}
                        </Button>
                        {needsReview(t) ? (
                          <Button
                            size="sm"
                            variant="ghost"
                            disabled={reviewing}
                            aria-label={copy.dataset.reject(t.title)}
                            onClick={() => review.mutate({ taskId: t.id, approve: false })}
                          >
                            {copy.dataset.rejectShort}
                          </Button>
                        ) : null}
                      </span>
                    ) : null
                  }
                >
                  <span className="min-w-0 truncate">{t.title}</span>
                  <span className="ml-auto shrink-0 text-xs text-muted-foreground">
                    {t.repo ? `${t.repo} · ` : ''}
                    {copy.dataset.status[t.status] ?? t.status}
                    {t.reject_reason ? ` · ${t.reject_reason}` : ''}
                  </span>
                </CheckRow>
              )
            })}
          </ul>
        </PickBox>
        <PickBox
          title={copy.dataset.models}
          hint={fromBenchmarks ? copy.dataset.benchmarkModelsHint : copy.dataset.modelsHint}
        >
          {dataset.models.length ? null : (
            <p className="px-2 text-sm text-muted-foreground">
              {copy.dataset.noModels}{' '}
              <Link to="/router" className="underline underline-offset-2">
                {copy.dataset.addModel}
              </Link>
            </p>
          )}
          <ul className="flex flex-col">
            {dataset.models.map((m) => (
              <CheckRow
                key={m.id}
                id={`model-${m.id}`}
                checked={picks.models.includes(m.id)}
                onChange={(on) => setPicks((p) => ({ ...p, models: toggle(p.models, m.id, on) }))}
              >
                <span className="truncate">{m.name}</span>
                {m.used ? <Badge variant="secondary">{copy.dataset.used}</Badge> : null}
              </CheckRow>
            ))}
          </ul>
        </PickBox>
      </div>
    </>
  )
}

const money = (n: number | null | undefined) => (n == null ? '–' : fmtMoneyFine(n))
const time = (n: number | null | undefined) => (n == null ? '–' : fmtLatency(n))

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
        {money(cost)} · {time(latency)}
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

/**
 * Each model's overall score (a bar, the best overall starred) as a carousel: one row of cards that scrolls sideways
 * and snaps to a card, a page at a time with the arrows. An edge with more cards past it fades out.
 *
 * ponytail: native scroll-snap, not shadcn's Carousel, which needs embla-carousel-react (a new dependency). Swap it in
 * if this wants drag-to-scroll with a mouse, looping or autoplay.
 */
function Scoreboard({ models }: { models: { name: string; totals: ModelTotals | null }[] }) {
  const top = Math.max(...models.map((m) => m.totals?.score ?? -Infinity))
  const {
    ref: track,
    before: canPrev,
    after: canNext,
    measure,
  } = useOverflow<HTMLUListElement>('x')
  const page = (dir: 1 | -1) => {
    const el = track.current
    el?.scrollBy({ left: dir * el.clientWidth })
  }
  return (
    <section aria-label={copy.results.byModel} className="mb-4">
      <div className="mb-2 flex min-h-8 items-center justify-between gap-2">
        <h3 className="text-sm font-medium">{copy.results.byModel}</h3>
        {canPrev || canNext ? (
          <div className="flex gap-1">
            <Button
              variant="outline"
              size="icon-sm"
              className="rounded-full"
              aria-label={copy.results.prevModels}
              disabled={!canPrev}
              onClick={() => page(-1)}
            >
              <ChevronLeft aria-hidden />
            </Button>
            <Button
              variant="outline"
              size="icon-sm"
              className="rounded-full"
              aria-label={copy.results.nextModels}
              disabled={!canNext}
              onClick={() => page(1)}
            >
              <ChevronRight aria-hidden />
            </Button>
          </div>
        ) : null}
      </div>
      <ul
        ref={track}
        onScroll={measure}
        className="flex snap-x snap-mandatory [scrollbar-width:none] gap-3 overflow-x-auto motion-safe:scroll-smooth [&::-webkit-scrollbar]:hidden"
        style={{
          maskImage: `linear-gradient(to right, ${canPrev ? 'transparent' : 'black'}, black 2rem, black calc(100% - 2rem), ${canNext ? 'transparent' : 'black'})`,
        }}
      >
        {models.map(({ name, totals }) => {
          const best = !!totals && totals.score === top
          return (
            <li
              key={name}
              className={cn(
                'flex shrink-0 basis-[85%] snap-start flex-col gap-2 rounded-lg border p-3 sm:basis-[46%] lg:basis-[31%] 2xl:basis-[23.5%]',
                best ? 'border-primary/40 bg-primary/5' : 'border-border',
              )}
            >
              <span className="flex items-center justify-between gap-2 text-sm font-medium">
                <span className="truncate">{name}</span>
                {best ? (
                  <Badge>
                    <Star className="size-3 fill-current" aria-hidden />
                    {copy.results.bestOverall}
                  </Badge>
                ) : null}
              </span>
              {totals ? (
                <>
                  <span className="text-2xl font-semibold tabular-nums">
                    {copy.results.score(totals.score)}
                  </span>
                  <Progress value={totals.score} aria-label={copy.results.scoreOf(name)} />
                  <span className="text-xs text-muted-foreground tabular-nums">
                    {money(totals.cost)} · {time(totals.latency)}
                  </span>
                </>
              ) : (
                <span className="text-sm text-muted-foreground">{copy.results.noResult}</span>
              )}
            </li>
          )
        })}
      </ul>
    </section>
  )
}

function Results({
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
  // A benchmark's tasks have no original session to compare with.
  const withOriginal = tasks.some((t) => t.original)
  const stale =
    !!picks && (key(taskIds) !== key(picks.tasks) || key(modelIds) !== key(picks.models))
  const going = run.status === 'queued' || run.status === 'running' || run.status === 'finalizing'
  return (
    <>
      {going ? (
        <Status>{copy.results.progress(run.progress.done, run.progress.total)}</Status>
      ) : run.status === 'failed' || run.status === 'cancelled' ? (
        <p role="alert" className="mb-3 text-sm text-destructive">
          {copy.results.ended(run.status)}
        </p>
      ) : null}
      {stale ? <Status>{copy.results.stale}</Status> : null}
      <Scoreboard
        models={run.models.map((m, i) => ({ name: m.name, totals: footer[i] ?? null }))}
      />
      <ScrollTable>
        <Table>
          <TableCaption className="sr-only">{copy.results.caption}</TableCaption>
          <TableHeader className={STICKY_HEAD}>
            <TableRow>
              <TableHead>{copy.results.task}</TableHead>
              {withOriginal ? (
                <TableHead className="text-right">{copy.results.original}</TableHead>
              ) : null}
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
                  {withOriginal ? (
                    <TableCell className="text-right">
                      {t?.original ? (
                        <div className="flex flex-col items-end gap-0.5">
                          <span className="text-sm">{t.original.model}</span>
                          <span className="text-xs text-muted-foreground tabular-nums">
                            {money(t.original.cost_usd)} · {time(t.original.latency_ms)}
                          </span>
                        </div>
                      ) : null}
                    </TableCell>
                  ) : null}
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
              {withOriginal ? <TableCell /> : null}
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
      </ScrollTable>
      <p className="mt-2 text-xs text-muted-foreground">{copy.results.caption}</p>
    </>
  )
}
