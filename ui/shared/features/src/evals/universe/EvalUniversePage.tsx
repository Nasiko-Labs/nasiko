/**
 * `/evals/universe` (`oss/docs/proposals/evals/CODE_REVIEW_EVALS.md`'s "Eval Universe" section):
 * every code review task and model attempt the caller has run, as a navigable 3D scene — one
 * flat, shared recall (X) / precision (Y) plane every attempt from every task plots on, points
 * colored by model identity. An earlier version used Z depth to separate tasks into lanes, but
 * with the usual 1-2 tasks that third dimension read as noise, not signal; which task a point
 * belongs to is carried by its hover tooltip and the sidebar's task filter instead. Reached from
 * Code review evals' own header link, not its own sidebar entry (`nav.ts`'s `/evals` item already
 * carries `also: ['/evals/code-review', '/evals/universe']`).
 *
 * Deliberately simple: a fixed layout (no force-directed physics), a flat capped list (no
 * pagination), the caller's own data only. See the proposal doc for the full list of what this
 * does not do.
 */
import { useMemo, useState } from 'react'
import { Html, OrbitControls, Stars, Text } from '@react-three/drei'
import { Canvas } from '@react-three/fiber'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { fmtLatency, fmtMoneyFine, fmtPct } from '@nasiko/ui/lib/format'
import { Card, CardContent, CardHeader, CardTitle } from '@nasiko/ui/primitives/card'
import { Checkbox } from '@nasiko/ui/primitives/checkbox'
import { Label } from '@nasiko/ui/primitives/label'
import { useUniverse } from './api'
import { copy } from './copy'
import { AXIS_SPAN, buildModelPalette, overlapOffset, pointColor, pointRadius, pointXY } from './scene'
import type { TaskSummary, UniverseAttempt, UniverseView } from './types'

const AXIS_TICKS = [0, 0.25, 0.5, 0.75, 1] as const

export function EvalUniversePage() {
  const universe = useUniverse()
  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-4">
      <TopBar />
      <PageHeader title={copy.title} description={copy.description} />
      {universe.isPending ? (
        <p>{copy.loading}</p>
      ) : universe.isError ? (
        <p className="text-destructive">{copy.failed}</p>
      ) : !universe.data.attempts.length ? (
        <p className="text-sm text-muted-foreground">{copy.empty}</p>
      ) : (
        <UniverseExplorer data={universe.data} />
      )}
    </div>
  )
}

// ── Scene ────────────────────────────────────────────────────────────────

function AttemptTooltip({ attempt, task }: { attempt: UniverseAttempt; task: TaskSummary }) {
  const tokens = (attempt.input_tokens ?? 0) + (attempt.output_tokens ?? 0)
  return (
    <div className="w-56 rounded-md border border-border bg-popover p-2 text-xs text-popover-foreground shadow-md">
      <p className="text-muted-foreground">
        {task.owner}/{task.repo} #{task.pr_number}
      </p>
      <p className="font-medium">{attempt.model_name}</p>
      <p className="text-muted-foreground">
        {attempt.provider}
        {attempt.model ? `: ${attempt.model}` : ''}
      </p>
      <p>
        {copy.tooltip.status}: {attempt.status}
      </p>
      {attempt.metrics ? (
        <>
          <p>
            {copy.tooltip.precision}: {fmtPct(attempt.metrics.precision * 100)}
          </p>
          <p>
            {copy.tooltip.recall}: {fmtPct(attempt.metrics.recall * 100)}
          </p>
        </>
      ) : (
        <p className="text-muted-foreground">{copy.tooltip.ungraded}</p>
      )}
      <p>
        {copy.tooltip.cost}: {fmtMoneyFine(attempt.cost_usd ?? 0)}
      </p>
      <p>
        {copy.tooltip.tokens}: {tokens}
      </p>
      <p>
        {copy.tooltip.latency}: {fmtLatency(attempt.latency_ms ?? 0)}
      </p>
    </div>
  )
}

function AttemptPoint({
  attempt,
  task,
  position,
  hovered,
  onHover,
  palette,
}: {
  attempt: UniverseAttempt
  task: TaskSummary
  position: [number, number, number]
  hovered: boolean
  onHover: (id: string | null) => void
  palette: Map<string, string>
}) {
  const color = pointColor(attempt, palette)
  const radius = pointRadius(attempt)
  return (
    <group position={position}>
      <mesh
        onPointerOver={(e) => {
          e.stopPropagation()
          onHover(attempt.attempt_id)
        }}
        onPointerOut={() => onHover(null)}
      >
        <sphereGeometry args={[radius, 24, 24]} />
        <meshStandardMaterial
          color={color}
          emissive={color}
          emissiveIntensity={hovered ? 0.9 : 0.25}
        />
      </mesh>
      {/* Label + full tooltip only while hovered — with every task's attempts sharing one plane,
       * an always-on label per point would overlap badly. Position already carries
       * precision/recall and color carries model identity; the label (and the tooltip's full
       * detail) is for the one point you're inspecting. Mesh-based `Text` (troika-three-text via
       * drei), not `Html` — see `PrecisionRecallAxes` for why `Html` can't be trusted here. */}
      {hovered ? (
        <>
          <Text
            position={[0, -(radius + 0.2), 0]}
            fontSize={0.16}
            color="#f8fafc"
            outlineWidth={0.012}
            outlineColor="#05070d"
            anchorX="center"
            anchorY="top"
          >
            {attempt.model_name}
          </Text>
          <Html distanceFactor={10} style={{ pointerEvents: 'none' }}>
            <AttemptTooltip attempt={attempt} task={task} />
          </Html>
        </>
      ) : null}
    </group>
  )
}

/**
 * The one shared recall (X) / precision (Y) axis pair every task's attempts plot against — drawn
 * once at the back of the scene, not per task, so every model's score is directly comparable on
 * the same scale. Grounded on a pedestal so it reads as a real chart corner, not a floating rule.
 * Uses drei's mesh-based `Text` (troika-three-text), not its DOM `Html` — `Html` loses its
 * children here, a known interaction with React `StrictMode`
 * (`shared/app-shell/src/mount.tsx`) double-invoking Canvas's own internal reconciler root on
 * first mount. `Text` renders as real geometry, so it isn't affected and can safely stay always-on
 * (the hover tooltip still uses `Html`, but only ever mounts once hovered, after that dance has
 * settled — see `AttemptTooltip` above).
 */
function PrecisionRecallAxes() {
  return (
    <group position={[0, 0, 0]}>
      {/* Floor pedestal under the whole plotting plane. */}
      <mesh position={[AXIS_SPAN / 2, -0.015, AXIS_SPAN / 2]} rotation={[-Math.PI / 2, 0, 0]}>
        <planeGeometry args={[AXIS_SPAN + 1.6, AXIS_SPAN + 1.6]} />
        <meshStandardMaterial color="#0f172a" transparent opacity={0.6} />
      </mesh>
      {/* Y axis: precision. */}
      <mesh position={[0, AXIS_SPAN / 2, 0]}>
        <cylinderGeometry args={[0.016, 0.016, AXIS_SPAN, 10]} />
        <meshStandardMaterial color="#64748b" emissive="#334155" emissiveIntensity={0.3} />
      </mesh>
      {/* X axis: recall. */}
      <mesh position={[AXIS_SPAN / 2, 0, 0]} rotation={[0, 0, Math.PI / 2]}>
        <cylinderGeometry args={[0.016, 0.016, AXIS_SPAN, 10]} />
        <meshStandardMaterial color="#64748b" emissive="#334155" emissiveIntensity={0.3} />
      </mesh>
      {AXIS_TICKS.map((t) => (
        <group key={`y-${t}`} position={[0, t * AXIS_SPAN, 0]}>
          <mesh>
            <sphereGeometry args={[0.04, 12, 12]} />
            <meshStandardMaterial color="#94a3b8" />
          </mesh>
          <Text
            position={[-0.26, 0, 0]}
            fontSize={0.14}
            color="#e2e8f0"
            outlineWidth={0.009}
            outlineColor="#05070d"
            anchorX="right"
            anchorY="middle"
          >
            {t.toFixed(2)}
          </Text>
        </group>
      ))}
      {AXIS_TICKS.map((t) => (
        <group key={`x-${t}`} position={[t * AXIS_SPAN, 0, 0]}>
          <mesh>
            <sphereGeometry args={[0.04, 12, 12]} />
            <meshStandardMaterial color="#94a3b8" />
          </mesh>
          <Text
            position={[0, -0.24, 0]}
            fontSize={0.14}
            color="#e2e8f0"
            outlineWidth={0.009}
            outlineColor="#05070d"
            anchorX="center"
            anchorY="top"
            rotation={[0, 0, 0]}
          >
            {t.toFixed(2)}
          </Text>
        </group>
      ))}
      <Text
        position={[-0.5, AXIS_SPAN + 0.3, 0]}
        fontSize={0.17}
        color="#e2e8f0"
        outlineWidth={0.009}
        outlineColor="#05070d"
        anchorX="left"
        anchorY="bottom"
      >
        precision (y)
      </Text>
      <Text
        position={[AXIS_SPAN + 0.2, -0.45, 0]}
        fontSize={0.17}
        color="#e2e8f0"
        outlineWidth={0.009}
        outlineColor="#05070d"
        anchorX="left"
        anchorY="middle"
      >
        recall (x)
      </Text>
    </group>
  )
}

/** Groups attempts sharing the same rounded (recall, precision) score, so `overlapOffset` only
 * ever has to fan out points that would otherwise sit exactly on top of each other. */
function groupByScore(attempts: UniverseAttempt[]): UniverseAttempt[][] {
  const groups = new Map<string, UniverseAttempt[]>()
  for (const a of attempts) {
    const [x, y] = pointXY(a)
    const key = `${x.toFixed(2)}:${y.toFixed(2)}`
    const g = groups.get(key)
    if (g) g.push(a)
    else groups.set(key, [a])
  }
  return Array.from(groups.values())
}

function Scene({
  tasksById,
  attempts,
}: {
  tasksById: Map<string, TaskSummary>
  attempts: UniverseAttempt[]
}) {
  const [hoveredId, setHoveredId] = useState<string | null>(null)
  const palette = useMemo(
    () => buildModelPalette(attempts.map((a) => a.model ?? a.model_name)),
    [attempts],
  )
  const groups = useMemo(() => groupByScore(attempts), [attempts])

  return (
    <Canvas camera={{ position: [1.5, 3, 7], fov: 50 }}>
      <color attach="background" args={['#05070d']} />
      <ambientLight intensity={0.6} />
      <pointLight position={[10, 10, 10]} intensity={1.2} />
      <gridHelper args={[60, 60, '#1e293b', '#141c2c']} />
      <Stars radius={60} depth={30} count={1500} factor={2} fade />
      <OrbitControls enableDamping dampingFactor={0.08} target={[AXIS_SPAN / 2, AXIS_SPAN / 3, 0]} />
      <PrecisionRecallAxes />
      {groups.map((group) =>
        group.map((a, i) => {
          const [x, y] = pointXY(a)
          const [dx, dy] = overlapOffset(i, group.length)
          const task = tasksById.get(a.task_id)
          if (!task) return null
          return (
            <AttemptPoint
              key={a.attempt_id}
              attempt={a}
              task={task}
              position={[x + dx, y + dy, 0]}
              hovered={hoveredId === a.attempt_id}
              onHover={setHoveredId}
              palette={palette}
            />
          )
        }),
      )}
    </Canvas>
  )
}

// ── Filters + page shell ─────────────────────────────────────────────────

function ModelLegend({ attempts }: { attempts: UniverseAttempt[] }) {
  const models = useMemo(() => {
    const seen = new Map<string, string>()
    for (const a of attempts) {
      const key = a.model ?? a.model_name
      if (!seen.has(key)) seen.set(key, a.model_name)
    }
    return Array.from(seen.entries())
  }, [attempts])
  const palette = useMemo(() => buildModelPalette(models.map(([key]) => key)), [models])
  if (!models.length) return null
  return (
    <ul className="flex flex-col gap-1 text-xs text-muted-foreground">
      {models.map(([key, name]) => (
        <li key={key} className="flex items-center gap-2">
          <span
            className="inline-block size-2.5 shrink-0 rounded-full"
            style={{ backgroundColor: palette.get(key) }}
          />
          {name}
        </li>
      ))}
    </ul>
  )
}

function UniverseExplorer({ data }: { data: UniverseView }) {
  const [providerOff, setProviderOff] = useState<Set<string>>(new Set())
  const [modelOff, setModelOff] = useState<Set<string>>(new Set())
  const [taskOff, setTaskOff] = useState<Set<string>>(new Set())
  const [gradedOnly, setGradedOnly] = useState(false)

  const providers = useMemo(
    () => Array.from(new Set(data.attempts.map((a) => a.provider))).sort(),
    [data.attempts],
  )
  const models = useMemo(
    () => Array.from(new Set(data.attempts.map((a) => a.model ?? a.model_name))).sort(),
    [data.attempts],
  )
  const tasksById = useMemo(() => new Map(data.tasks.map((t) => [t.id, t])), [data.tasks])

  const filtered = useMemo(
    () =>
      data.attempts.filter((a) => {
        if (providerOff.has(a.provider)) return false
        if (modelOff.has(a.model ?? a.model_name)) return false
        if (taskOff.has(a.task_id)) return false
        if (gradedOnly && a.metrics == null) return false
        return true
      }),
    [data.attempts, providerOff, modelOff, taskOff, gradedOnly],
  )

  const toggle = (set: Set<string>, setSet: (s: Set<string>) => void, key: string, on: boolean) => {
    const next = new Set(set)
    if (on) next.delete(key)
    else next.add(key)
    setSet(next)
  }

  return (
    <div className="grid gap-4 lg:grid-cols-[1fr_16rem]">
      <Card>
        <CardContent className="flex flex-col gap-2 p-3">
          <div className="h-[36rem] overflow-hidden rounded-lg">
            <Scene tasksById={tasksById} attempts={filtered} />
          </div>
        </CardContent>
      </Card>
      <div className="flex flex-col gap-4">
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">{copy.legend.title}</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-1 text-xs text-muted-foreground">
            <p>{copy.legend.axes}</p>
            <p>{copy.legend.color}</p>
            <p>{copy.legend.size}</p>
            <p>{copy.legend.labels}</p>
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">{copy.legend.modelsTitle}</CardTitle>
          </CardHeader>
          <CardContent>
            <ModelLegend attempts={filtered} />
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle className="text-sm">{copy.filters.title}</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <div className="flex flex-col gap-1">
              <Label className="text-xs text-muted-foreground">{copy.filters.task}</Label>
              {data.tasks.map((t) => (
                <label key={t.id} className="flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={!taskOff.has(t.id)}
                    onCheckedChange={(on) => toggle(taskOff, setTaskOff, t.id, !!on)}
                  />
                  {t.owner}/{t.repo} #{t.pr_number}
                </label>
              ))}
            </div>
            <div className="flex flex-col gap-1">
              <Label className="text-xs text-muted-foreground">{copy.filters.provider}</Label>
              {providers.map((p) => (
                <label key={p} className="flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={!providerOff.has(p)}
                    onCheckedChange={(on) => toggle(providerOff, setProviderOff, p, !!on)}
                  />
                  {p}
                </label>
              ))}
            </div>
            <div className="flex flex-col gap-1">
              <Label className="text-xs text-muted-foreground">{copy.filters.model}</Label>
              {models.map((m) => (
                <label key={m} className="flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={!modelOff.has(m)}
                    onCheckedChange={(on) => toggle(modelOff, setModelOff, m, !!on)}
                  />
                  {m}
                </label>
              ))}
            </div>
            <label className="flex items-center gap-2 text-sm">
              <Checkbox checked={gradedOnly} onCheckedChange={(on) => setGradedOnly(!!on)} />
              {copy.filters.gradedOnly}
            </label>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
