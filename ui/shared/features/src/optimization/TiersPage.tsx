/**
 * Settings → Workspace → Optimization tiers (plans/feat-context-optimization.md §4; design review 2B; sketch v2 5),
 * superuser only. What Low, Medium and High mean, in three numbered cards of the Settings rows.
 * - Today's server (no CX-T1 route): the known defaults as plain text with their variable names, under "Defaults. Your
 *   server's environment may override these." Nothing pretends to be the running value.
 * - With the route: the same rows as inputs, one Save for the set (PUT guarded by `updated_at`), the rules in
 *   `tierProblems`. A reply missing any value stays read-only (ledger V2): the page never writes back a value it didn't
 *   read. History compression sits in Shared limits, with the one Save in that card's footer (V4).
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useId, useState, type ReactNode } from 'react'
import { Controller, useForm, useWatch } from 'react-hook-form'
import { toast } from 'sonner'
import { LeaveGuard } from '@nasiko/app-core/leave-guard'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { PanelError } from '@nasiko/ui/composites/panel'
import { Alert, AlertDescription } from '@nasiko/ui/primitives/alert'
import { Button } from '@nasiko/ui/primitives/button'
import { Input } from '@nasiko/ui/primitives/input'
import { Switch } from '@nasiko/ui/primitives/switch'
import {
  SettingHeading,
  SettingRow,
  SettingRows,
} from '@nasiko/features/settings/components/SettingRow'
import { ApiError } from '@nasiko/data-access/client'
import { tiersQuery, useSaveTiers } from './api'
import { copy } from './copy'
import { numberVisible, tierProblems, type TierField } from './logic'
import { LEVELS, SERVER_DEFAULT_TIERS, type FullTiers, type Level, type Tiers } from './types'

const c = copy.tiers
type Unit = keyof typeof c.unit

/** One value row: label, hint with its variable, and the value (or its input). */
interface RowSpec {
  field: TierField
  label: string
  hint: string
  env: string
  unit: Unit
}

const levelLabel = (l: Level) => copy.level(l)
const SECTIONS: { key: 'budgets' | 'counts' | 'shared'; title: string; rows: RowSpec[] }[] = [
  {
    key: 'budgets',
    title: c.budgets,
    rows: LEVELS.map((l) => ({
      field: `pacms_budget.${l}` as const,
      label: levelLabel(l),
      hint: c.budgetHint[l],
      env: `PACMS_BUDGET_${l.toUpperCase()}`,
      unit: 'tokens' as const,
    })),
  },
  {
    key: 'counts',
    title: c.counts,
    rows: LEVELS.map((l) => ({
      field: `context_k.${l}` as const,
      label: levelLabel(l),
      hint: c.countHint[l],
      env: `CONTEXT_K_${l.toUpperCase()}`,
      unit: 'messages' as const,
    })),
  },
  {
    key: 'shared',
    title: c.shared,
    rows: [
      {
        field: 'pool_size',
        label: c.pool,
        hint: c.poolHint,
        env: 'PACMS_HISTORY_POOL_SIZE',
        unit: 'messages',
      },
      {
        field: 'mandatory_recent',
        label: c.kept,
        hint: c.keptHint,
        env: 'PACMS_HISTORY_MANDATORY_RECENT',
        unit: 'messages',
      },
      {
        field: 'compress_min_bytes',
        label: c.compressOver,
        hint: c.compressOverHint,
        env: 'TOKEN_COMPRESS_HISTORY_MIN_BYTES',
        unit: 'bytes',
      },
    ],
  },
]

const read = (t: FullTiers, f: TierField): number => {
  const [a, b] = f.split('.') as [keyof FullTiers, Level | undefined]
  const v = t[a]
  return b && typeof v === 'object' ? v[b] : (v as number)
}

/** The reply carries every value and its write guard: only then is the page editable (ledger V2). */
const complete = (t: Tiers): t is FullTiers & { updated_at: string } =>
  t.pool_size !== undefined &&
  t.mandatory_recent !== undefined &&
  t.compress_min_bytes !== undefined &&
  t.updated_at !== undefined

/** For display only: the values the reply has, the defaults for the rest. */
const shown = (t: Tiers): FullTiers => ({
  pacms_budget: t.pacms_budget,
  context_k: t.context_k,
  compress_history: t.compress_history,
  pool_size: t.pool_size ?? SERVER_DEFAULT_TIERS.pool_size,
  mandatory_recent: t.mandatory_recent ?? SERVER_DEFAULT_TIERS.mandatory_recent,
  compress_min_bytes: t.compress_min_bytes ?? SERVER_DEFAULT_TIERS.compress_min_bytes,
})

const Env = ({ name }: { name: string }) => <span className="font-mono text-xs"> {name}</span>

export function TiersPage() {
  const tiers = useQuery(tiersQuery)
  const [generation, setGeneration] = useState(0)
  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={c.title} description={c.sub} />
      {tiers.isPending ? (
        <PageLoader label={c.loading} />
      ) : tiers.data === undefined ? (
        // Only when nothing loaded: a failed background refetch keeps the form and its draft.
        <PanelError error={tiers.error} onRetry={() => void tiers.refetch()} what={c.what} />
      ) : tiers.data === null ? (
        <ReadOnlyView values={SERVER_DEFAULT_TIERS} title={c.defaultsTitle} hint={c.defaultsHint} />
      ) : !complete(tiers.data) ? (
        <ReadOnlyView values={shown(tiers.data)} title={c.partialTitle} hint={c.partialHint} />
      ) : (
        <TiersForm
          key={generation}
          saved={tiers.data}
          onSaved={() => setGeneration((g) => g + 1)}
        />
      )}
    </div>
  )
}

/** The numbered sections; `last` ends Shared limits (History compression), `footer` is that card's Save. */
function Sections({
  row,
  last,
  footer,
}: {
  row: (r: RowSpec) => ReactNode
  last: ReactNode
  footer?: ReactNode
}) {
  const numbered = numberVisible(SECTIONS.map((s) => ({ key: s.key, shown: true })))
  const end = SECTIONS.length - 1
  return (
    <>
      {SECTIONS.map((s, i) => (
        <section key={s.key} aria-labelledby={`tiers-${s.key}`}>
          <SettingHeading>
            <span id={`tiers-${s.key}`}>{copy.section(numbered[i].n, s.title)}</span>
          </SettingHeading>
          <SettingRows footer={i === end ? footer : undefined}>
            {s.rows.map(row)}
            {i === end ? last : null}
          </SettingRows>
        </section>
      ))}
    </>
  )
}

const rowHint = (r: RowSpec) => (
  <>
    {r.hint}
    <Env name={r.env} />
  </>
)

const compressHint = (
  <>
    {c.compressHistoryHint}
    <Env name="TOKEN_COMPRESS_HISTORY" />
  </>
)

/**
 * 2B: values as plain text with their variables. Today's server: the built-in defaults, labelled as such; a reply
 * missing a value: what it has, read-only (V2).
 */
function ReadOnlyView({
  values: t,
  title,
  hint,
}: {
  values: FullTiers
  title: string
  hint: string
}) {
  return (
    <div className="flex flex-col">
      <div className="rounded-lg border border-border bg-muted/40 px-4 py-3 text-sm">
        <p className="font-medium">{title}</p>
        <p className="mt-1 text-muted-foreground">{hint}</p>
      </div>
      <Sections
        row={(r) => (
          <SettingRow key={r.field} label={r.label} hint={rowHint(r)}>
            <p className="text-sm tabular-nums">{c.value(read(t, r.field), c.unit[r.unit])}</p>
          </SettingRow>
        )}
        last={
          <SettingRow label={c.compressHistory} hint={compressHint}>
            <p className="text-sm">{t.compress_history ? c.on : c.off}</p>
          </SettingRow>
        }
      />
    </div>
  )
}

/** A form key without dots: react-hook-form reads `a.b` as a nested path, which would split the draft in two. */
type FormKey =
  | `${'pacms_budget' | 'context_k'}__${Level}`
  | 'pool_size'
  | 'mandatory_recent'
  | 'compress_min_bytes'
const keyOf = (f: TierField) => f.replace('.', '__') as FormKey

type Draft = Record<FormKey, string> & { compress_history: boolean }

const FIELDS: TierField[] = SECTIONS.flatMap((s) => s.rows.map((r) => r.field))
const KEYS: FormKey[] = FIELDS.map(keyOf)

const toDraft = (t: FullTiers): Draft => {
  const d = { compress_history: t.compress_history } as Draft
  for (const f of FIELDS) d[keyOf(f)] = String(read(t, f))
  return d
}

const fromDraft = (d: Draft): FullTiers => {
  const n = (f: TierField) => {
    const v = d[keyOf(f)] ?? ''
    return v.trim() === '' ? Number.NaN : Number(v)
  }
  return {
    pacms_budget: {
      low: n('pacms_budget.low'),
      medium: n('pacms_budget.medium'),
      high: n('pacms_budget.high'),
    },
    context_k: {
      low: n('context_k.low'),
      medium: n('context_k.medium'),
      high: n('context_k.high'),
    },
    pool_size: n('pool_size'),
    mandatory_recent: n('mandatory_recent'),
    compress_min_bytes: n('compress_min_bytes'),
    compress_history: d.compress_history,
  }
}

/** With the CX-T1 route: the same rows as inputs, one Save (remounted after, never reset()). */
function TiersForm({
  saved,
  onSaved,
}: {
  saved: FullTiers & { updated_at: string }
  onSaved: () => void
}) {
  const id = useId()
  const save = useSaveTiers()
  // What this form loaded, pinned at mount: a refetch (after a 409 or a failed save) moves `saved`, but the draft, its
  // dirty check and the write guard must stay on the version the inputs were filled from.
  const [loaded] = useState(saved)
  const form = useForm<Draft>({ defaultValues: toDraft(loaded) })
  const [problems, setProblems] = useState<Partial<Record<TierField, string>>>({})
  // Another save landed after this form loaded (409): the draft stays until the user loads the new values.
  const [conflict, setConflict] = useState(false)
  const qc = useQueryClient()
  // Reads the newer values before dropping the draft: a failed read keeps it, so nothing is lost to a bad network.
  const loadTheirs = async () => {
    try {
      const fresh = await qc.fetchQuery({ ...tiersQuery, staleTime: 0 })
      if (fresh && fresh.updated_at !== loaded.updated_at) return onSaved()
    } catch {
      // falls through to the message below
    }
    toast.error(c.conflictLoadFailed)
  }
  // Compared field by field with what loaded (formState.isDirty isn't reliable under the React Compiler).
  const initial = toDraft(loaded)
  // Named fields: a whole-form useWatch hands back one object that native inputs mutate in place, so a memoised
  // render never sees it change.
  const values = useWatch({ control: form.control, name: KEYS })
  const compress = useWatch({ control: form.control, name: 'compress_history' })
  const isDirty =
    compress !== initial.compress_history || KEYS.some((k, i) => (values[i] ?? '') !== initial[k])

  const submit = form.handleSubmit((d) => {
    const t = fromDraft(d)
    const found = tierProblems(t)
    setProblems(Object.fromEntries(Object.entries(found).map(([k, p]) => [k, c.problem[p]])))
    if (Object.keys(found).length) return
    save.mutate(
      { tiers: t, expectedUpdatedAt: loaded.updated_at },
      {
        onSuccess: () => {
          toast.success(c.saved)
          onSaved()
        },
        onError: (e) => {
          if (e instanceof ApiError && e.status === 409) setConflict(true)
          else toast.error((e instanceof ApiError && e.serverMessage) || copy.saveFailed)
        },
      },
    )
  })

  return (
    <form onSubmit={(e) => void submit(e)} noValidate className="flex flex-col">
      {conflict ? (
        <Alert className="mb-2">
          <AlertDescription className="flex flex-wrap items-center gap-x-3 gap-y-2">
            {c.conflict}
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="pointer-coarse:min-h-11"
              onClick={() => void loadTheirs()}
            >
              {c.conflictAction}
            </Button>
          </AlertDescription>
        </Alert>
      ) : null}
      <Sections
        row={(r) => {
          const fid = `${id}-${r.field}`
          return (
            <SettingRow
              key={r.field}
              htmlFor={fid}
              label={r.label}
              hint={rowHint(r)}
              hintId={`${fid}-hint`}
              error={problems[r.field]}
            >
              <div className="flex items-center gap-2">
                <Input
                  id={fid}
                  type="number"
                  inputMode="numeric"
                  min={1}
                  step={1}
                  aria-invalid={!!problems[r.field]}
                  aria-describedby={`${fid}-hint`}
                  {...form.register(keyOf(r.field), {
                    onChange: () => setProblems((p) => ({ ...p, [r.field]: undefined })),
                  })}
                />
                <span className="w-16 shrink-0 text-sm text-muted-foreground">
                  {c.unit[r.unit]}
                </span>
              </div>
            </SettingRow>
          )
        }}
        last={
          <SettingRow
            htmlFor={`${id}-compress`}
            label={c.compressHistory}
            hint={compressHint}
            hintId={`${id}-compress-hint`}
          >
            <Controller
              control={form.control}
              name="compress_history"
              render={({ field }) => (
                <Switch
                  id={`${id}-compress`}
                  checked={field.value}
                  onCheckedChange={field.onChange}
                  aria-describedby={`${id}-compress-hint`}
                />
              )}
            />
          </SettingRow>
        }
        footer={
          <Button
            type="submit"
            size="sm"
            className="pointer-coarse:min-h-11"
            disabled={!isDirty || save.isPending || conflict}
          >
            {save.isPending ? copy.saving : copy.save}
          </Button>
        }
      />
      <LeaveGuard when={isDirty && !save.isPending} />
    </form>
  )
}
