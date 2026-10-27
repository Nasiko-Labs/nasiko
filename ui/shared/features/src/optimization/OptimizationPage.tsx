/**
 * Your settings on /optimization (plans/feat-context-optimization.md §3; plans/feat-optimization-page.md P2, P10, R2D,
 * R2G, E4, C8): how much past conversation each new message carries, for every signed-in user, OSS and EE. The page's
 * "Your settings" section carries the heading; Settings → Account → Optimization redirects there.
 *
 * One card of numbered rows (1A: only shown rows are numbered) with Save in its footer, like every Settings page:
 * - Context optimization switch: only when the server reports `enabled` (CX-5); the intro says "always on" otherwise.
 * - Context strategy: tiles built like Appearance's (a RadioGroup, the radio inside each tile's label).
 * - History budget: a ToggleGroup, its figure only with the server's tier values (2B), and the last-chat preview
 *   (CX-6, follows the draft strategy, 2F) when the server has it; it runs only once the section is on screen (E4).
 * Compression isn't a row here (P10): the lead states it and Needs attention fixes it.
 * Save sends only what changed, both routes at once; a field that fails keeps its draft (2C). While anything is unsaved
 * the Save bar sticks to the bottom of the viewport (R2G); LeaveGuard lets the page's own search moves through (C8).
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Fragment, useEffect, useId, useState, type ReactNode } from 'react'
import { Controller, useForm, useWatch } from 'react-hook-form'
import { toast } from 'sonner'
import { LeaveGuard } from '@nasiko/app-core/leave-guard'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { PanelError } from '@nasiko/ui/composites/panel'
import { Button } from '@nasiko/ui/primitives/button'
import { Label } from '@nasiko/ui/primitives/label'
import { RadioGroup, RadioGroupItem } from '@nasiko/ui/primitives/radio-group'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { Switch } from '@nasiko/ui/primitives/switch'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import { ToggleGroup, ToggleGroupItem } from '@nasiko/ui/primitives/toggle-group'
import { Tooltip, TooltipContent, TooltipTrigger } from '@nasiko/ui/primitives/tooltip'
import { Chosen } from '@nasiko/features/settings/components/Chosen'
import { SettingRow, SettingRows } from '@nasiko/features/settings/components/SettingRow'
import { PICTURE, TILE } from '@nasiko/features/settings/components/tileStyles'
import { cn } from '@nasiko/ui/lib/utils'
import { budgetQuery, previewQuery, strategyQuery, tiersQuery, useSavePreferences } from './api'
import { copy } from './copy'
import {
  approx,
  changedFields,
  numberVisible,
  tierFigure,
  type PreferenceEdit,
  type SaveOutcome,
} from './logic'
import { LEVELS, STRATEGIES, type Level, type Strategy } from './types'

type Values = Required<PreferenceEdit>

/** `previewEnabled`: the last-chat preview may run (E4: the section has been on screen, or #settings was opened). */
export function OptimizationPage({ previewEnabled }: { previewEnabled: boolean }) {
  const strategy = useQuery(strategyQuery)
  const budget = useQuery(budgetQuery)
  // Remounted after each Save with the next draft (never reset(): CLAUDE.md Settings).
  const [form, setForm] = useState<{
    generation: number
    draft: Values | null
    errors: SaveOutcome['failed']
  }>({
    generation: 0,
    draft: null,
    errors: [],
  })
  const withSwitch = strategy.data?.enabled !== undefined
  const loading = strategy.isPending || budget.isPending
  // An error with cached data (a background refetch) keeps the form; only a first load that failed swaps it out.
  const failed = (!strategy.data && strategy.error) || (!budget.data && budget.error) || null
  return (
    <div className="flex flex-col gap-6">
      {/* Without the switch (today's server) the one fact the form can't show: it is always on (1A). */}
      {strategy.data && !withSwitch ? (
        <p className="-mt-3 text-sm text-muted-foreground">{copy.subAlwaysOn}</p>
      ) : null}
      {failed ? (
        <PanelError
          error={failed}
          what={copy.title.toLowerCase()}
          onRetry={() => void Promise.all([strategy.refetch(), budget.refetch()])}
        />
      ) : loading || !strategy.data || !budget.data ? (
        <PageLoader label={copy.loading} />
      ) : (
        <PreferencesForm
          key={form.generation}
          saved={{
            strategy: strategy.data.strategy,
            level: budget.data.level,
            enabled: strategy.data.enabled ?? true,
          }}
          initial={form.draft}
          initialErrors={form.errors}
          withSwitch={withSwitch}
          previewEnabled={previewEnabled}
          onSaved={(submitted, outcome) =>
            setForm((f) => ({
              generation: f.generation + 1,
              // 2C: what saved is the server's value now and what failed is still the draft, so either way the remount
              // starts from what was submitted; with nothing failed it starts from the refetched server values.
              draft: outcome.failed.length ? submitted : null,
              errors: outcome.failed,
            }))
          }
        />
      )}
    </div>
  )
}

function PreferencesForm({
  saved,
  initial,
  initialErrors,
  withSwitch,
  previewEnabled,
  onSaved,
}: {
  saved: Values
  /** The draft a failed field kept (2C); the server's values otherwise. */
  initial: Values | null
  initialErrors: SaveOutcome['failed']
  withSwitch: boolean
  previewEnabled: boolean
  onSaved: (submitted: Values, outcome: SaveOutcome) => void
}) {
  const id = useId()
  const save = useSavePreferences(copy.saveFailed)
  const tiers = useQuery(tiersQuery)
  const form = useForm<Values>({ defaultValues: initial ?? saved })
  // The server's values when this form mounted: a refetch that brings another tab's change must not turn an untouched
  // field into an edit that Save would write back.
  const [base] = useState(saved)
  const draft = useWatch({ control: form.control }) as Values
  const edit = changedFields(base, draft, withSwitch)
  const dirty = Object.keys(edit).length > 0
  const errorFor = (field: keyof Values) => {
    const e = initialErrors.find((x) => x.field === field)
    return e && edit[field] !== undefined
      ? copy.fieldFailed(copy.fieldName[field], e.message)
      : undefined
  }

  const submit = form.handleSubmit((v) => {
    const changes = changedFields(base, v, withSwitch)
    save.mutate(
      { edit: changes, strategy: v.strategy },
      {
        onSuccess: (outcome) => {
          if (outcome.saved.length)
            toast.success(copy.saved(outcome.saved.map((f) => copy.fieldName[f])))
          onSaved(v, outcome)
        },
      },
    )
  })

  const sections = numberVisible([
    { key: 'switch' as const, shown: withSwitch },
    { key: 'strategy' as const, shown: true },
    { key: 'budget' as const, shown: true },
  ])
  const off = withSwitch && !draft.enabled
  const figure = tierFigure(tiers.data ?? undefined, draft.strategy, draft.level)

  const body: Record<(typeof sections)[number]['key'], (n: number) => ReactNode> = {
    switch: (n) => (
      <SettingRow
        htmlFor={`${id}-enabled`}
        label={copy.section(n, copy.switchTitle)}
        hint={off ? copy.switchOffNote : copy.switchHint}
        hintId={`${id}-enabled-hint`}
        error={errorFor('enabled')}
      >
        <Controller
          control={form.control}
          name="enabled"
          render={({ field }) => (
            // The row's control column is 20rem (sized for inputs); a switch sits at its far edge, by the card's.
            <div className="flex @[640px]:justify-end">
              <Switch
                id={`${id}-enabled`}
                checked={field.value}
                onCheckedChange={field.onChange}
                aria-describedby={`${id}-enabled-hint`}
                aria-invalid={!!errorFor('enabled')}
              />
            </div>
          )}
        />
      </SettingRow>
    ),
    strategy: (n) => (
      <SettingRow
        label={copy.section(n, copy.strategyTitle)}
        hint={copy.strategyHint}
        hintId={`${id}-strategy-hint`}
        error={errorFor('strategy')}
        stacked
      >
        <Controller
          control={form.control}
          name="strategy"
          render={({ field }) => (
            <RadioGroup
              aria-label={copy.strategyTitle}
              aria-describedby={`${id}-strategy-hint`}
              value={field.value}
              onValueChange={(v) => field.onChange(v as Strategy)}
              disabled={off}
              aria-invalid={!!errorFor('strategy')}
              // 6A: tiles stack until the card is 640 px wide (explicit grid-cols-1: the overflow pitfall).
              className="grid grid-cols-1 gap-3 @[640px]:grid-cols-3 @[640px]:gap-4"
            >
              {STRATEGIES.map((s) => (
                <Label key={s} className={cn(TILE, off && 'cursor-not-allowed opacity-60')}>
                  {/* The name heads the tile and is the radio's name; the words under it describe the choice. */}
                  <span
                    className={cn(
                      PICTURE,
                      'flex flex-col gap-1 bg-card p-3 pr-8 @[640px]:min-h-24',
                    )}
                  >
                    <span className="font-medium text-foreground">
                      <RadioGroupItem
                        value={s}
                        className="sr-only"
                        aria-describedby={`${id}-${s}-desc`}
                      />
                      {copy.strategies[s].label}
                    </span>
                    <span id={`${id}-${s}-desc`} aria-hidden className="text-muted-foreground">
                      {copy.strategies[s].hint}
                    </span>
                    <Chosen />
                  </span>
                </Label>
              ))}
            </RadioGroup>
          )}
        />
      </SettingRow>
    ),
    budget: (n) => (
      <SettingRow
        label={copy.section(n, copy.budgetTitle)}
        hint={
          <>
            {copy.budgetHint(draft.strategy)}
            {figure ? ` ${copy.figure(draft.level, figure.kind, figure.value)}` : null}
          </>
        }
        hintId={`${id}-budget-hint`}
        error={errorFor('level')}
        stacked
      >
        <Controller
          control={form.control}
          name="level"
          render={({ field }) => (
            <ToggleGroup
              variant="outline"
              size="sm"
              value={[field.value]}
              onValueChange={(vs) => {
                const v = vs[0] as string
                return v && field.onChange(v as Level)
              }}
              aria-label={copy.budgetTitle}
              aria-describedby={`${id}-budget-hint`}
              aria-invalid={!!errorFor('level')}
              disabled={off}
              // 6B: as on the Overview, arrows stop at the ends.
              loopFocus={false}
              className="w-fit"
            >
              {LEVELS.map((l) => (
                <ToggleGroupItem key={l} value={l} className="px-3 text-xs pointer-coarse:min-h-11">
                  {copy.level(l)}
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
          )}
        />
        {off ? null : (
          <LastChatPreview
            strategy={draft.strategy}
            current={base.level}
            enabled={previewEnabled}
          />
        )}
      </SettingRow>
    ),
  }

  return (
    <form onSubmit={(e) => void submit(e)} noValidate>
      <SettingRows
        stickyFooter={dirty}
        footer={
          <>
            {dirty ? (
              <span className="mr-auto text-sm text-muted-foreground">{copy.unsaved}</span>
            ) : null}
            <Button
              type="submit"
              size="sm"
              className="pointer-coarse:min-h-11"
              disabled={!dirty || save.isPending}
            >
              {save.isPending ? copy.saving : copy.save}
            </Button>
          </>
        }
      >
        {sections.map((s) => (
          <Fragment key={s.key}>{body[s.key](s.n)}</Fragment>
        ))}
      </SettingRows>
      {/* C8: a sort, window change or bar click keeps the draft; leaving the page still asks. */}
      <LeaveGuard when={dirty && !save.isPending} samePath />
    </form>
  )
}

/** CX-6 (2F, E6): what each tier keeps from the last chat, for the draft strategy; hidden when the server lacks it. */
function LastChatPreview({
  strategy,
  current,
  enabled,
}: {
  strategy: Strategy
  current: Level
  enabled: boolean
}) {
  // E4: a dry run spends embedding calls, so it waits until the section is on screen (the skeleton holds its place).
  const preview = useQuery({ ...previewQuery(strategy), enabled })
  // E6: the previews last for the page visit, every strategy's; leaving drops them so the next visit reads the latest chat.
  const queryClient = useQueryClient()
  useEffect(
    () => () => queryClient.removeQueries({ queryKey: ['optimization', 'preview'] }),
    [queryClient],
  )
  if (preview.isPending)
    return (
      // 2F: a table-shaped skeleton (header + 3 tiers + baseline would jump less than one block).
      <div aria-hidden className="flex flex-col gap-2" data-testid="preview-loading">
        {[0, 1, 2, 3].map((i) => (
          <Skeleton key={i} className="h-8 w-full rounded-md" />
        ))}
      </div>
    )
  if (preview.isError)
    return (
      <p className="text-sm text-muted-foreground">
        {copy.preview.failed}{' '}
        <Button
          // Inside the form: a plain button, or Retry would also submit it.
          type="button"
          variant="link"
          size="sm"
          className="h-auto p-0 pointer-coarse:min-h-11"
          onClick={() => void preview.refetch()}
        >
          {copy.retry}
        </Button>
      </p>
    )
  const r = preview.data
  if (r.kind === 'absent') return null
  if (r.kind === 'no-session')
    return <p className="text-sm text-muted-foreground">{copy.preview.noChats}</p>
  const p = r.preview
  const n = (v: number) => `~${approx(v).toLocaleString('en-US')}`
  // Message counts are exact; only tokens are estimates.
  const exact = (v: number) => v.toLocaleString('en-US')
  return (
    <div className="flex flex-col gap-1.5">
      <p className="text-sm text-muted-foreground">
        {copy.preview.caption(p.session_messages, strategy)}
      </p>
      <Table aria-label={copy.preview.label}>
        <TableHeader>
          <TableRow>
            <TableHead>{copy.preview.tier}</TableHead>
            <TableHead className="text-right">
              <span className="@[640px]:hidden">{copy.preview.keptShort}</span>
              <span className="hidden @[640px]:inline">{copy.preview.kept}</span>
            </TableHead>
            <TableHead className="hidden text-right @[640px]:table-cell">
              {copy.preview.tokens}
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {LEVELS.map((l) => (
            <TableRow key={l}>
              <TableCell>
                {copy.level(l)}
                {l === current ? (
                  <span className="text-muted-foreground"> {copy.preview.current}</span>
                ) : null}
              </TableCell>
              <TableCell className="text-right tabular-nums">
                {exact(p.tiers[l].messages)}
                {/* 6A: one "~12 · ~980" cell until the card is 640 px wide. */}
                <span className="@[640px]:hidden"> · {n(p.tiers[l].tokens)}</span>
              </TableCell>
              <TableCell className="hidden text-right tabular-nums @[640px]:table-cell">
                {n(p.tiers[l].tokens)}
              </TableCell>
            </TableRow>
          ))}
          <TableRow>
            <TableCell className="text-muted-foreground">
              <Tooltip>
                <TooltipTrigger
                  render={
                    <span
                      // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- focusable only so keyboard users can open the baseline tooltip; it has no action, so no button role
                      tabIndex={0}
                      className="underline decoration-dotted underline-offset-4"
                    />
                  }
                >
                  {copy.preview.without}
                </TooltipTrigger>
                <TooltipContent className="max-w-xs">{copy.preview.baselineTip}</TooltipContent>
              </Tooltip>
            </TableCell>
            <TableCell className="text-right text-muted-foreground tabular-nums">
              {p.pool.messages.toLocaleString('en-US')}
              <span className="@[640px]:hidden"> · {n(p.pool.tokens)}</span>
            </TableCell>
            <TableCell className="hidden text-right text-muted-foreground tabular-nums @[640px]:table-cell">
              {n(p.pool.tokens)}
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
  )
}
