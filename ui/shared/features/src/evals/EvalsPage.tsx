/**
 * `/evals` (docs/superpowers/specs/2026-11-23-evals-design.md): Claude Code's evals, as a stepper. The first step is
 * the dataset's source: tick sessions, or, when Claude Code has none, pick pre-defined SWE benchmarks. Then pick tasks
 * and models from the dataset, run, and compare. A step opens once what it shows exists. Not connected: the way to
 * connect it.
 *
 * ponytail: the step, the picks, the dataset and the results live in page state, not the URL: no server stores
 * datasets yet, and mock state doesn't survive a reload. Put the step and the dataset id in the URL once one does.
 */
import { Link } from '@tanstack/react-router'
import { ChevronLeft, ChevronRight, FlaskConical, Star } from 'lucide-react'
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { errorCode, isEndpointAbsent } from '@nasiko/data-access/detect'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { EmptyState, StateCard } from '@nasiko/ui/composites/state-card'
import { Stepper } from '@nasiko/ui/composites/stepper'
import { fmtInt, fmtLatency, fmtLocalDateTime, fmtMoneyFine } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
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
import { useEvalBenchmarks, useEvalSessions, useGenerateDataset, useRunEvals } from './api'
import { copy } from './copy'
import { comparison, type ModelTotals } from './logic'
import { evalSteps, STEP, type Source, type Step } from './steps'
import type { EvalBenchmark, EvalDataset, EvalResult, EvalSession } from './types'

/** ponytail: Claude Code is the one harness evals support; a harness step comes back with a second one. */
const HARNESS = 'claude'

interface Picks {
  tasks: string[]
  models: string[]
}

const key = (ids: readonly string[]) => [...ids].sort().join('\n')
const toggle = (ids: readonly string[], id: string, on: boolean) =>
  on ? [...ids, id] : ids.filter((x) => x !== id)

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
    <li className="flex min-h-9 items-center gap-2.5 rounded-md px-2 py-1 hover:bg-muted/50">
      <Checkbox id={id} checked={checked} onCheckedChange={onChange} />
      <Label htmlFor={id} className="flex min-w-0 flex-1 items-center gap-2 font-normal">
        {children}
      </Label>
    </li>
  )
}

export function EvalsPage() {
  const sessions = useEvalSessions(HARNESS)
  const benchmarks = useEvalBenchmarks(sessions.isSuccess && !sessions.data.length)
  const generate = useGenerateDataset()
  const run = useRunEvals()
  const [step, setStep] = useState<Step>(STEP.source)
  // The ticked sessions, or the picked benchmarks: whichever the first step offers.
  const [ticked, setTicked] = useState<string[]>([])
  const [picks, setPicks] = useState<Picks>({ tasks: [], models: [] })

  // Narrower than `max-w-page`: a step's tables and cards read better a little tighter (the user, 2026-11-24).
  const frame = (children: ReactNode) => (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-4">
      <TopBar />
      <PageHeader title={copy.title} description={copy.description} />
      {children}
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
  // A dataset belongs to what it was built from: tick something else and it's gone until it is built again.
  const builtFrom = generate.variables?.session_ids ?? generate.variables?.benchmark_ids
  const dataset =
    generate.data && builtFrom && key(builtFrom) === key(ticked) ? generate.data : null
  const ran =
    dataset && run.data && run.variables?.datasetId === dataset.id
      ? { ...run.variables, results: run.data.results }
      : null
  const stale =
    !!ran && (key(ran.task_ids) !== key(picks.tasks) || key(ran.model_ids) !== key(picks.models))
  // The step asked for, unless what it shows is gone: then the last step that still has something.
  const shown: Step =
    step === STEP.compare && ran
      ? STEP.compare
      : step !== STEP.source && dataset
        ? STEP.dataset
        : STEP.source

  const canOpen = (i: number) =>
    i === STEP.source || (i === STEP.dataset && !!dataset) || (i === STEP.compare && !!ran)

  const onGenerate = () => {
    run.reset()
    generate.mutate(
      source === 'sessions'
        ? { harness: HARNESS, session_ids: ticked }
        : { harness: HARNESS, benchmark_ids: ticked },
      {
        onSuccess: (d) => {
          setPicks({
            tasks: d.tasks.map((t) => t.id),
            models: d.models.filter((m) => m.used).map((m) => m.id),
          })
          setStep(STEP.dataset)
        },
      },
    )
  }
  const onRun = () => {
    if (!dataset) return
    run.mutate(
      { datasetId: dataset.id, task_ids: picks.tasks, model_ids: picks.models },
      { onSuccess: () => setStep(STEP.compare) },
    )
  }

  const action =
    shown === STEP.source ? (
      <Button disabled={!ticked.length || generate.isPending} onClick={onGenerate}>
        {source === 'benchmarks'
          ? generate.isPending
            ? copy.opening
            : copy.goToDataset
          : generate.isPending
            ? copy.generating
            : generate.data && !dataset
              ? copy.regenerate(ticked.length)
              : copy.generate(ticked.length)}
      </Button>
    ) : shown === STEP.dataset ? (
      <Button
        disabled={!picks.tasks.length || !picks.models.length || run.isPending}
        onClick={onRun}
      >
        {run.isPending ? copy.running : copy.run}
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
        dataset: dataset ? copy.dataset.picked(picks.tasks.length, picks.models.length) : undefined,
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
            <SessionsTable rows={sessions.data} ticked={ticked} setTicked={setTicked} />
            {generate.isError ? <Failed>{copy.generateFailed}</Failed> : null}
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
          </>
        )
      ) : shown === STEP.dataset && dataset ? (
        <>
          <StepHeading title={copy.dataset.title} hint={copy.dataset.hint} />
          <DatasetPicks dataset={dataset} source={source} picks={picks} setPicks={setPicks} />
          {run.isError ? <Failed>{copy.runFailed}</Failed> : null}
        </>
      ) : dataset && ran ? (
        <>
          <StepHeading
            title={copy.results.title}
            hint={stale ? copy.results.stale : copy.results.caption}
          />
          <Results
            dataset={dataset}
            taskIds={ran.task_ids}
            modelIds={ran.model_ids}
            results={ran.results}
          />
        </>
      ) : null}
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
  const all = ticked.length === rows.length
  return (
    <ScrollTable>
      <Table>
        <TableHeader className={STICKY_HEAD}>
          <TableRow>
            <TableHead className="w-10">
              <Checkbox
                aria-label={copy.sessions.all}
                checked={all}
                onCheckedChange={(on) => setTicked(() => (on ? rows.map((s) => s.session_id) : []))}
              />
            </TableHead>
            <TableHead>{copy.sessions.cols.title}</TableHead>
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
                    onCheckedChange={(v) => setTicked((t) => toggle(t, s.session_id, v))}
                  />
                </TableCell>
                <TableCell className="max-w-0 min-w-56 truncate font-medium">{s.title}</TableCell>
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

function DatasetPicks({
  dataset,
  source,
  picks,
  setPicks,
}: {
  dataset: EvalDataset
  source: Source
  picks: Picks
  setPicks: (f: (p: Picks) => Picks) => void
}) {
  const fromSessions = source === 'sessions'
  return (
    <div className="grid gap-4 md:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
      <PickBox
        title={copy.dataset.tasks}
        hint={fromSessions ? copy.dataset.tasksHint : copy.dataset.benchmarkTasksHint}
      >
        <ul className="flex max-h-80 flex-col overflow-y-auto">
          {dataset.tasks.map((t) => (
            <CheckRow
              key={t.id}
              id={`task-${t.id}`}
              checked={picks.tasks.includes(t.id)}
              onChange={(on) => setPicks((p) => ({ ...p, tasks: toggle(p.tasks, t.id, on) }))}
            >
              <span className="truncate">{t.title}</span>
            </CheckRow>
          ))}
        </ul>
      </PickBox>
      <PickBox
        title={copy.dataset.models}
        hint={fromSessions ? copy.dataset.modelsHint : copy.dataset.benchmarkModelsHint}
      >
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
                    {fmtMoneyFine(totals.cost)} · {fmtLatency(totals.latency)}
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
  taskIds,
  modelIds,
  results,
}: {
  dataset: EvalDataset
  taskIds: string[]
  modelIds: string[]
  results: EvalResult[]
}) {
  const { rows, footer } = comparison(taskIds, modelIds, results)
  const taskTitle = new Map(dataset.tasks.map((t) => [t.id, t.title]))
  const modelName = new Map(dataset.models.map((m) => [m.id, m.name]))
  const nameOf = (m: string) => modelName.get(m) ?? m
  return (
    <>
      <Scoreboard
        models={modelIds.map((m, i) => ({ name: nameOf(m), totals: footer[i] ?? null }))}
      />
      <ScrollTable>
        <Table>
          <TableCaption className="sr-only">{copy.results.caption}</TableCaption>
          <TableHeader className={STICKY_HEAD}>
            <TableRow>
              <TableHead>{copy.results.task}</TableHead>
              {modelIds.map((m) => (
                <TableHead key={m} className="text-right">
                  {nameOf(m)}
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
      </ScrollTable>
    </>
  )
}
