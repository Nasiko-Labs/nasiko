/**
 * The agent routing sheet (plan §4.5): configured routing, then "Route this agent with" (default or a config) and a
 * separate Override block, one Save. The save runs `requestPlan` as ONE mutation (eng #2): attach or detach first
 * (both clear the pin on the server), then the pin. A failed step is reported with what was saved; the agent's row
 * is re-read either way.
 */
import { zodResolver } from '@hookform/resolvers/zod'
import { Link } from '@tanstack/react-router'
import { AlertTriangle } from 'lucide-react'
import { useEffect, useId, useMemo, useState } from 'react'
import { Controller, useForm, useWatch } from 'react-hook-form'
import { z } from 'zod'
import { CopyButton } from '@/components/shared/copy-button'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Field, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { useAnnounce } from '../announce'
import { useReturnFocus } from '../focus'
import { useFreshAgentRouting, useRoutingPlan, type PlanOutcome } from '../api'
import { copy } from '../copy'
import { routerError } from '../errors'
import { fmtLocalTime } from '@/lib/format'
import { sourceLabel } from '../format'
import {
  catalogIndex,
  cliAttach,
  cliDetach,
  keepOverrideCheck,
  keySource,
  parseFallback,
  routingSentence,
  type RoutingState,
  type Step,
} from '../routing'
import { CONFIG_CACHE_SECONDS } from '../tuning'
import type { AgentRouting, LlmConfig, ProviderCatalog } from '../types'
import { LeaveGuard } from '@/components/shared/leave-guard'
import { KeySourceChip, SentenceText } from './bits'

/** The routing form: 'default' or a config id, the override model, and whether a route change keeps the override. */
const schema = z.object({ choice: z.string(), override: z.string(), keep: z.boolean() })
type Form = z.infer<typeof schema>

export interface RoutingTarget {
  id: string
  name: string
  displayName: string
  ownerId: string
}

export function RoutingSheet({
  target,
  viewer,
  configs,
  catalog,
  onClose,
  onSaved,
}: {
  target: RoutingTarget | null
  viewer: { sub?: string; superuser: boolean }
  /** The viewer's own configs; only meaningful when the viewer owns the agent. */
  configs: LlmConfig[] | undefined
  catalog: ProviderCatalog[] | undefined
  onClose: () => void
  onSaved: (agentId: string, at: number) => void
}) {
  const returnFocus = useReturnFocus(!!target)
  return (
    <Sheet
      open={!!target}
      onOpenChange={(o) => {
        if (!o) onClose()
      }}
    >
      <SheetContent className="w-full gap-0 sm:max-w-sheet" onCloseAutoFocus={returnFocus}>
        {target ? (
          <Fresh
            key={target.id}
            target={target}
            viewer={viewer}
            configs={configs}
            catalog={catalog}
            onClose={onClose}
            onSaved={onSaved}
          />
        ) : null}
      </SheetContent>
    </Sheet>
  )
}

/** Waits for the read the sheet started to settle, then seeds the form from it (once). */
function Fresh(props: Omit<Parameters<typeof Body>[0], 'routing'>) {
  const q = useFreshAgentRouting(props.target.id)
  const [settled, setSettled] = useState(false)
  if (!settled && q.data && !q.isFetching) setSettled(true)
  if (settled && q.data) return <Body {...props} routing={q.data} />
  return (
    <SheetHeader>
      <SheetTitle>{copy.routingTitle(props.target.displayName)}</SheetTitle>
      <SheetDescription>
        {q.isError && !q.isFetching ? copy.couldntRead : copy.readingRouting}
      </SheetDescription>
      {q.isError && !q.isFetching ? (
        <Button
          size="sm"
          variant="outline"
          className="w-fit pointer-coarse:min-h-11"
          onClick={() => void q.refetch()}
        >
          {copy.retry}
        </Button>
      ) : null}
    </SheetHeader>
  )
}

const stepText = (s: Step) =>
  s.kind === 'attach'
    ? copy.stepAttach
    : s.kind === 'detach'
      ? copy.stepDetach
      : s.kind === 'pin'
        ? copy.stepPin
        : copy.stepUnpin

function Body({
  target,
  routing,
  viewer,
  configs,
  catalog,
  onClose,
  onSaved,
}: {
  target: RoutingTarget
  routing: AgentRouting
  viewer: { sub?: string; superuser: boolean }
  configs: LlmConfig[] | undefined
  catalog: ProviderCatalog[] | undefined
  onClose: () => void
  onSaved: (agentId: string, at: number) => void
}) {
  const ids = useId()
  const announce = useAnnounce()
  const [outcome, setOutcome] = useState<(PlanOutcome & { at: number }) | null>(null)
  const plan = useRoutingPlan(target.id, (out) => {
    const at = Date.now()
    if (out.saved.length) onSaved(target.id, at)
    announce(
      out.failed ? routerError(out.failed.error).problem : copy.savedPlan(target.displayName),
    )
  })
  const isOwner = viewer.sub === target.ownerId
  // A superuser on someone else's agent sees only their own configs, never the owner's (R-L7).
  const canReadConfigs = isOwner
  const idx = useMemo(() => catalogIndex(catalog), [catalog])
  const ownerConfigs = canReadConfigs ? (configs ?? []) : []
  const defaultConfig =
    routing.source === 'owner-default'
      ? routing.llm_config
      : (ownerConfigs.find((c) => c.is_default) ?? null)
  // Seeded once from the routing at open (never re-seeded under the user).
  const {
    control,
    register,
    setValue,
    formState: { isDirty },
  } = useForm<Form>({
    defaultValues: {
      choice: routing.llm_config_id ?? 'default',
      override: routing.pinned_model ?? '',
      keep: true,
    },
    resolver: zodResolver(schema),
  })
  const { choice, override, keep } = useWatch({ control }) as Form
  const setChoice = (v: string) => setValue('choice', v, { shouldDirty: true })
  const current: RoutingState = {
    llm_config_id: routing.llm_config_id,
    pinned_model: routing.pinned_model,
  }
  const desiredConfig = choice === 'default' ? null : choice
  const routeChanged = desiredConfig !== routing.llm_config_id
  const hadPin = !!routing.pinned_model
  const targetConfig = desiredConfig
    ? (ownerConfigs.find((c) => c.id === desiredConfig) ?? null)
    : defaultConfig
  const keepCheck = routing.pinned_model
    ? keepOverrideCheck(routing.pinned_model, targetConfig, idx, canReadConfigs)
    : null
  const keepAllowed = !!keepCheck?.allowed
  const pinUnchanged = override === (routing.pinned_model ?? '')
  const desiredPin =
    routeChanged && hadPin && pinUnchanged
      ? keep && keepAllowed
        ? routing.pinned_model
        : null
      : override.trim() || null
  const desired: RoutingState = { llm_config_id: desiredConfig, pinned_model: desiredPin }
  const noChange =
    desired.llm_config_id === current.llm_config_id && desired.pinned_model === current.pinned_model
  const { reset } = plan
  useEffect(() => () => reset(), [reset])

  const run = (from: RoutingState, to: RoutingState) => {
    plan.mutate(
      { current: from, desired: to },
      {
        onSuccess: (out) => setOutcome({ ...out, at: Date.now() }),
      },
    )
  }

  const sentence = routingSentence(routing.llm_config, routing.pinned_model)
  const cfg = routing.llm_config

  return (
    <>
      <SheetHeader className="border-b border-border">
        <SheetTitle>{copy.routingTitle(target.displayName)}</SheetTitle>
        <SheetDescription>{copy.configuredNote}</SheetDescription>
      </SheetHeader>
      <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4 text-sm">
        <dl className="grid grid-cols-[7rem_minmax(0,1fr)] gap-x-3 gap-y-2">
          <dt className="text-muted-foreground">{copy.labelSource}</dt>
          <dd>{sourceLabel(routing.source, isOwner)}</dd>
          <dt className="text-muted-foreground">{copy.labelProvider}</dt>
          <dd>{cfg?.provider ?? copy.none}</dd>
          <dt className="text-muted-foreground">{copy.labelModel}</dt>
          <dd>
            <SentenceText sentence={sentence} full />
          </dd>
          <dt className="text-muted-foreground">{copy.labelKey}</dt>
          <dd>
            <KeySourceChip source={keySource(cfg)} />
          </dd>
          <dt className="text-muted-foreground">{copy.labelFallbacks}</dt>
          <dd>
            {cfg?.fallback_models.length ? (
              <ul className="space-y-1">
                {cfg.fallback_models.map((f) => {
                  const p = parseFallback(f, cfg)
                  return (
                    <li key={f}>
                      <span className="font-mono text-xs">{f}</span>
                      {p.crossProvider ? (
                        <span className="block text-xs text-muted-foreground">
                          {copy.platformKeyFallback(p.provider)}
                        </span>
                      ) : null}
                    </li>
                  )
                })}
              </ul>
            ) : (
              copy.none
            )}
          </dd>
          <dt className="text-muted-foreground">{copy.labelOverride}</dt>
          <dd className="font-mono text-xs">
            {routing.pinned_model ?? <span className="font-sans text-sm">{copy.none}</span>}
          </dd>
          <dt className="text-muted-foreground">{copy.requestFormat}</dt>
          <dd>
            <span className="font-mono text-xs">{routing.inbound_format}</span>
            <span className="block text-xs text-muted-foreground">
              {copy.requestFormatWayOut(target.name)}
            </span>
          </dd>
        </dl>

        {outcome ? (
          <Outcome
            outcome={outcome}
            target={target}
            onPinAgain={(model) => {
              // Plan from what the server returned after the saved steps, not the routing the sheet opened with.
              const now = outcome.after
                ? {
                    llm_config_id: outcome.after.llm_config_id,
                    pinned_model: outcome.after.pinned_model,
                  }
                : current
              run(now, { llm_config_id: now.llm_config_id, pinned_model: model })
            }}
            pending={plan.isPending}
          />
        ) : (
          <>
            <LeaveGuard when={isDirty && !plan.isPending} />
            <FieldSet className="gap-2">
              <FieldLegend variant="label" className="mb-0">
                {copy.routeWith}
              </FieldLegend>
              <RadioGroup
                aria-label={copy.routeWith}
                value={choice === 'default' ? 'default' : 'config'}
                className="gap-2"
                onValueChange={(v) =>
                  setChoice(
                    v === 'default'
                      ? 'default'
                      : (ownerConfigs.find((c) => c.id === routing.llm_config_id)?.id ??
                          ownerConfigs[0]?.id ??
                          'default'),
                  )
                }
              >
                <div className="flex items-center gap-2">
                  <RadioGroupItem id={`${ids}-default`} value="default" />
                  <Label htmlFor={`${ids}-default`} className="font-normal">
                    {isOwner ? copy.useMyDefault : copy.useOwnersDefault}
                  </Label>
                </div>
                <div
                  className={`flex flex-wrap items-center gap-2 ${canReadConfigs ? '' : 'opacity-60'}`}
                >
                  <RadioGroupItem
                    id={`${ids}-config`}
                    value="config"
                    disabled={!canReadConfigs || !ownerConfigs.length}
                  />
                  <Label htmlFor={`${ids}-config`} className="font-normal">
                    {copy.useAConfig}
                  </Label>
                  {canReadConfigs ? (
                    <Select
                      value={choice === 'default' ? '' : choice}
                      onValueChange={setChoice}
                      disabled={!ownerConfigs.length}
                    >
                      <SelectTrigger
                        size="sm"
                        aria-label={copy.chooseConfig}
                        className="pointer-coarse:min-h-11"
                      >
                        <SelectValue placeholder={copy.chooseConfig} />
                      </SelectTrigger>
                      <SelectContent>
                        {ownerConfigs.map((c) => (
                          <SelectItem key={c.id} value={c.id}>
                            {c.name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  ) : null}
                </div>
              </RadioGroup>
              {!canReadConfigs ? (
                <p className="text-xs text-muted-foreground">{copy.cantSeeOwnerConfigs}</p>
              ) : null}
              {routeChanged && hadPin && pinUnchanged ? (
                <div className="space-y-1 rounded-md border border-border p-2">
                  <p className="flex items-center gap-1.5">
                    <AlertTriangle className="size-3.5 text-warning" aria-hidden />{' '}
                    {copy.overrideWarning}
                  </p>
                  <Controller
                    control={control}
                    name="keep"
                    render={({ field }) => (
                      <Field
                        orientation="horizontal"
                        className={`gap-2 ${keepAllowed ? '' : 'opacity-60'}`}
                      >
                        <Checkbox
                          id={`${ids}-keep`}
                          ref={field.ref}
                          checked={field.value && keepAllowed}
                          disabled={!keepAllowed}
                          onCheckedChange={(c) => field.onChange(c === true)}
                        />
                        <FieldLabel htmlFor={`${ids}-keep`} className="font-normal">
                          {copy.keepOverride}
                        </FieldLabel>
                      </Field>
                    )}
                  />
                  {keepCheck && !keepCheck.allowed ? (
                    <p className="text-xs text-muted-foreground">{keepCheck.reason}</p>
                  ) : null}
                  {keepCheck?.allowed && keepCheck.anyProvider ? (
                    <p className="text-xs text-muted-foreground">{copy.keepAnyProvider}</p>
                  ) : null}
                </div>
              ) : null}
            </FieldSet>
            <FieldSet className="gap-1.5">
              <FieldLegend variant="label" className="mb-0">
                {copy.overrideSection}
              </FieldLegend>
              <FieldLabel
                htmlFor={`${ids}-override`}
                className="text-xs font-normal text-muted-foreground"
              >
                {copy.overrideModel}
              </FieldLabel>
              <div className="flex gap-2">
                <Input
                  id={`${ids}-override`}
                  list={`${ids}-models`}
                  {...register('override')}
                  placeholder={copy.overridePlaceholder}
                />
                <datalist id={`${ids}-models`}>
                  {(catalog ?? []).flatMap((g) =>
                    g.models.map((m) => (
                      <option key={`${g.provider}/${m.model}`} value={m.model} />
                    )),
                  )}
                </datalist>
                {override ? (
                  <Button
                    className="pointer-coarse:min-h-11"
                    type="button"
                    variant="ghost"
                    onClick={() => setValue('override', '', { shouldDirty: true })}
                  >
                    {copy.removeOverride}
                  </Button>
                ) : null}
              </div>
            </FieldSet>
            <div className="space-y-1 text-xs text-muted-foreground">
              <p>{copy.cliEquivalent}</p>
              {desiredConfig ? (
                <CopyButton
                  text={cliAttach(
                    target.name,
                    ownerConfigs.find((c) => c.id === desiredConfig)?.name ?? desiredConfig,
                  )}
                  label={copy.copyCli}
                  showText
                />
              ) : (
                <CopyButton text={cliDetach(target.name)} label={copy.copyCli} showText />
              )}
            </div>
          </>
        )}
      </div>
      <SheetFooter className="sticky bottom-0 border-t border-border bg-background">
        {outcome ? (
          <div className="flex justify-end pointer-coarse:min-h-11">
            <Button onClick={onClose}>{copy.done}</Button>
          </div>
        ) : (
          <>
            <p className="text-xs text-muted-foreground">{copy.timing}</p>
            <div className="flex justify-end gap-2">
              <Button className="pointer-coarse:min-h-11" variant="ghost" onClick={onClose}>
                {copy.cancel}
              </Button>
              <Button
                className="pointer-coarse:min-h-11"
                disabled={noChange || plan.isPending}
                onClick={() => run(current, desired)}
              >
                {plan.isPending ? copy.saving : copy.save}
              </Button>
            </div>
          </>
        )}
      </SheetFooter>
    </>
  )
}

function Outcome({
  outcome,
  target,
  onPinAgain,
  pending,
}: {
  outcome: PlanOutcome & { at: number }
  target: RoutingTarget
  onPinAgain: (model: string) => void
  pending: boolean
}) {
  const by = fmtLocalTime(outcome.at + CONFIG_CACHE_SECONDS * 1000)
  const failed = outcome.failed
  const repin =
    failed &&
    failed.step.kind === 'pin' &&
    outcome.saved.some((s) => s.kind === 'attach' || s.kind === 'detach')
  return (
    <div className="space-y-2">
      {!failed ? (
        <p className="font-medium">{copy.savedPlan(target.displayName)}</p>
      ) : repin ? (
        <div className="space-y-1">
          <p className="text-warning">
            {outcome.saved.some((st) => st.kind === 'attach')
              ? copy.partialOverride(String(failed.step.body.pinned_model))
              : copy.partialOverrideDefault(String(failed.step.body.pinned_model))}
          </p>
          <Button
            className="pointer-coarse:min-h-11"
            size="sm"
            variant="outline"
            disabled={pending}
            onClick={() => onPinAgain(String(failed.step.body.pinned_model))}
          >
            {copy.pinAgain}
          </Button>
        </div>
      ) : (
        <div className="space-y-1 text-destructive">
          <p>
            {outcome.saved.length
              ? copy.midwayFailed(outcome.saved.map(stepText).join(', '))
              : copy.nothingSaved}
          </p>
          <p>
            {routerError(failed.error).problem} {routerError(failed.error).action}
          </p>
        </div>
      )}
      {outcome.saved.length ? (
        <>
          <p className="text-muted-foreground">{copy.checkHint(by)}</p>
          <p className="text-muted-foreground">{copy.cantConfirm}</p>
          <Link
            to="/sessions"
            search={{ agent: target.name }}
            className="text-primary-text underline-offset-4 hover:underline"
          >
            {copy.seeSessions}
          </Link>
        </>
      ) : null}
    </div>
  )
}
