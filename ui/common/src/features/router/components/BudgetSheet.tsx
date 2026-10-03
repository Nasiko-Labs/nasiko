/**
 * The budget sheet (plans/feat-llm-router.md §5.1, approved variant A): scope (new budgets), monthly limit, threshold
 * chips (default 50 / 80 / 100) and the action at 100%. Choosing Stop calls states the consequence inline, naming the
 * scope and the reset date; there is no confirm dialog (no overlay on a sheet). Removing a budget confirms inline too.
 */
import { zodResolver } from '@hookform/resolvers/zod'
import { X } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { useForm, useWatch } from 'react-hook-form'
import { z } from 'zod'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Field, FieldDescription, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field'
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
import { fmtMoney, fmtShortDay } from '@/lib/format'
import { cn } from '@/lib/utils'
import { useAnnounce } from '../announce'
import { useDeleteBudget, useSaveBudget, type SaveBudget } from '../api'
import {
  budgetLabel,
  DEFAULT_THRESHOLDS,
  limitError,
  openScopes,
  parseLimit,
  parseThreshold,
  sentenceName,
  thresholdError,
  withStopMark,
} from '../budgets'
import { copy } from '../copy'
import { routerError } from '../errors'
import { fieldErrors } from '../form'
import { useReturnFocus } from '../focus'
import type { Budget, BudgetAction, BudgetStatus } from '../types'
import { LeaveGuard } from '@/components/shared/leave-guard'
import { LinkButton, Warn } from './bits'

/** The budget form. Scope is '' until picked ("owner" or an agent id); a new budget with no open scope can't save. */
const schema = z.object({
  scope: z.string(),
  limit: z.string().superRefine((v, ctx) => {
    const e = limitError(v)
    if (e) ctx.addIssue({ code: 'custom', message: e })
  }),
  thresholds: z.array(z.number()).superRefine((v, ctx) => {
    const e = thresholdError(v)
    if (e) ctx.addIssue({ code: 'custom', message: e })
  }),
  action: z.enum(['alert', 'stop']),
})
type Form = z.infer<typeof schema>

export type BudgetMode = { kind: 'create' } | { kind: 'edit'; budget: Budget }

interface SheetProps {
  mode: BudgetMode
  budgets: readonly Budget[]
  agents: readonly { id: string; name: string }[]
  agentName: (id: string) => string | undefined
  status?: BudgetStatus
  resetsAt: string
  onClose: () => void
}

export function BudgetSheet({
  mode,
  ...props
}: Omit<SheetProps, 'mode'> & { mode: BudgetMode | null }) {
  const returnFocus = useReturnFocus(!!mode)
  return (
    <Sheet
      open={!!mode}
      onOpenChange={(o) => {
        if (!o) props.onClose()
      }}
    >
      <SheetContent className="w-full gap-0 sm:max-w-sheet-sm" onCloseAutoFocus={returnFocus}>
        {mode ? (
          <Body key={mode.kind === 'edit' ? mode.budget.id : 'new'} {...props} mode={mode} />
        ) : null}
      </SheetContent>
    </Sheet>
  )
}

function Body({ mode, budgets, agents, agentName, status, resetsAt, onClose }: SheetProps) {
  const ids = useId()
  const announce = useAnnounce()
  const save = useSaveBudget()
  const del = useDeleteBudget()
  const src = mode.kind === 'edit' ? mode.budget : null
  const open = openScopes(budgets, agents)
  const firstScope = open.owner ? 'owner' : (open.agents[0]?.id ?? '')
  const {
    control,
    register,
    setValue,
    handleSubmit,
    formState: { isDirty, isSubmitting, touchedFields, submitCount },
  } = useForm<Form>({
    defaultValues: {
      scope: src ? (src.scope === 'owner' ? 'owner' : (src.agent_id ?? '')) : '',
      limit: src ? src.limit_usd.toFixed(2) : '',
      thresholds: src ? [...src.thresholds] : [...DEFAULT_THRESHOLDS],
      action: src?.action ?? 'alert',
    },
    resolver: zodResolver(schema),
  })
  const form = useWatch({ control }) as Form
  const { limit, thresholds, action } = form
  // Opened before the owned agents loaded: take the first open scope once they arrive.
  const scope = form.scope || firstScope
  const setThresholds = (t: number[]) => setValue('thresholds', t, { shouldDirty: true })
  const [draft, setDraft] = useState('')
  // Errors show once the limit was left or a save was tried.
  const touched = !!touchedFields.limit || submitCount > 0
  const [confirmingDelete, setConfirmingDelete] = useState(false)
  const confirmRef = useRef<HTMLParagraphElement>(null)
  const removeRef = useRef<HTMLButtonElement>(null)
  const [returnToRemove, setReturnToRemove] = useState(false)
  // The inline confirm replaces the Remove button: move focus into it, and back when it's dismissed.
  useEffect(() => {
    if (confirmingDelete) confirmRef.current?.focus()
    else if (returnToRemove) removeRef.current?.focus()
  }, [confirmingDelete, returnToRemove])
  const [error, setError] = useState<unknown>(null)

  const label = src
    ? budgetLabel(src, agentName)
    : scope === 'owner'
      ? copy.budgetOwner
      : (agents.find((a) => a.id === scope)?.name ?? '')
  // For sentences: null words it as "your budget".
  const subject = src ? sentenceName(src, agentName) : scope === 'owner' ? null : label
  const reset = fmtShortDay(resetsAt)
  const errs = fieldErrors(schema, form)
  const limitErr = errs.limit
  const thrErr = errs.thresholds
  const noScope = !src && !scope
  const invalid = !!limitErr || !!thrErr || noScope
  const busy = save.isPending || del.isPending || isSubmitting

  const draftValue = parseThreshold(draft)
  const addDraft = () => {
    if (draftValue === null || thresholds.includes(draftValue)) return
    setThresholds([...thresholds, draftValue].sort((a, b) => a - b))
    setDraft('')
  }
  const chooseAction = (a: BudgetAction) => {
    setValue('action', a, { shouldDirty: true })
    // A stop budget always alerts at 100 (R-L10).
    setThresholds(withStopMark(thresholds, a))
  }
  const submit = () => {
    setError(null)
    // A mark typed but not added still counts: Save shouldn't silently drop it.
    const marks = withStopMark(
      draftValue !== null && !thresholds.includes(draftValue)
        ? [...thresholds, draftValue].sort((a, b) => a - b)
        : thresholds,
      action,
    )
    const body = { limit_usd: parseLimit(limit), thresholds: marks, action }
    const v: SaveBudget = src
      ? { mode: 'update', id: src.id, body: { ...body, expected_updated_at: src.updated_at } }
      : {
          mode: 'create',
          body: {
            ...body,
            scope: scope === 'owner' ? 'owner' : 'agent',
            agent_id: scope === 'owner' ? null : scope,
          },
        }
    save.mutate(v, {
      onSuccess: () => {
        announce(copy.budgetSaved(subject))
        onClose()
      },
      onError: (e) => setError(e),
    })
  }
  const remove = () => {
    if (!src) return
    setError(null)
    del.mutate(src.id, {
      onSuccess: () => {
        announce(copy.budgetDeleted(subject))
        onClose()
      },
      onError: (e) => setError(e),
    })
  }
  const errView = error ? routerError(error) : null

  return (
    <>
      <SheetHeader className="border-b border-border">
        <SheetTitle>{src ? copy.budgetEdit(subject) : copy.budgetNew}</SheetTitle>
        <SheetDescription>
          {src && status
            ? copy.budgetStatusLine(fmtMoney(status.used_usd), fmtMoney(src.limit_usd), reset)
            : copy.budgetDescription}
        </SheetDescription>
      </SheetHeader>
      <form
        className="flex min-h-0 flex-1 flex-col"
        noValidate
        onSubmit={(e) => {
          if (busy) e.preventDefault()
          else void handleSubmit(submit)(e)
        }}
      >
        <LeaveGuard when={isDirty && !busy} />
        <div className="min-h-0 flex-1 space-y-5 overflow-y-auto p-4 text-sm">
          {!src ? (
            <Field className="gap-1">
              <FieldLabel htmlFor={`${ids}-scope`}>{copy.budgetScope}</FieldLabel>
              {noScope ? (
                <p className="text-muted-foreground">{copy.budgetNoScopes}</p>
              ) : (
                <>
                  <Select
                    value={scope}
                    onValueChange={(v) => setValue('scope', v, { shouldDirty: true })}
                  >
                    <SelectTrigger id={`${ids}-scope`} className="w-full pointer-coarse:min-h-11">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {open.owner ? (
                        <SelectItem value="owner">{copy.budgetOwnerOption}</SelectItem>
                      ) : null}
                      {open.agents.map((a) => (
                        <SelectItem key={a.id} value={a.id}>
                          {a.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <FieldDescription className="text-xs">
                    {scope === 'owner' ? copy.budgetScopeOwnerHint : copy.budgetScopeAgentHint}
                  </FieldDescription>
                </>
              )}
            </Field>
          ) : null}

          <Field className="gap-1">
            <FieldLabel htmlFor={`${ids}-limit`}>{copy.monthlyLimit}</FieldLabel>
            <Input
              id={`${ids}-limit`}
              inputMode="decimal"
              // eslint-disable-next-line jsx-a11y/no-autofocus -- Raise limit opens the sheet to edit the limit, so focus starts there (RouterBudgets.test)
              autoFocus={!!src}
              {...register('limit')}
              aria-invalid={(touched && !!limitErr) || errView?.field === 'limit'}
              aria-describedby={touched && limitErr ? `${ids}-limit-err` : undefined}
            />
            {touched && limitErr ? (
              <p id={`${ids}-limit-err`} className="text-xs text-destructive">
                {limitErr}
              </p>
            ) : null}
          </Field>

          <FieldSet
            className="gap-2"
            aria-describedby={
              thrErr || errView?.field === 'thresholds' ? `${ids}-thr-err` : undefined
            }
          >
            <FieldLegend variant="label" className="mb-0">
              {copy.alertAt}
            </FieldLegend>
            <div className="flex flex-wrap items-center gap-2">
              {thresholds.map((t) => (
                <Badge
                  key={t}
                  variant="muted"
                  className={cn(
                    'gap-1 py-0.5 text-sm font-normal text-foreground',
                    action === 'stop' && t === 100 ? 'px-2.5' : 'pr-1 pl-2.5',
                  )}
                >
                  {t}%
                  {action === 'stop' && t === 100 ? null : (
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-xs"
                      className="size-5 rounded-full text-muted-foreground hover:bg-transparent hover:text-foreground pointer-coarse:size-11"
                      onClick={() => setThresholds(thresholds.filter((x) => x !== t))}
                      aria-label={copy.removeThreshold(t)}
                    >
                      <X className="size-3.5" aria-hidden />
                    </Button>
                  )}
                </Badge>
              ))}
            </div>
            <div className="flex gap-2">
              <Input
                aria-label={copy.addThreshold}
                className="w-28"
                inputMode="numeric"
                placeholder={copy.thresholdPlaceholder}
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault()
                    addDraft()
                  }
                }}
              />
              <Button
                type="button"
                variant="outline"
                className="pointer-coarse:min-h-11"
                disabled={draftValue === null || thresholds.includes(draftValue)}
                onClick={addDraft}
              >
                {copy.addThreshold}
              </Button>
            </div>
            <p className="text-xs text-muted-foreground">{copy.alertAtHint}</p>
            {action === 'stop' ? (
              <p className="text-xs text-muted-foreground">{copy.stopKeeps100}</p>
            ) : null}
            {thrErr || errView?.field === 'thresholds' ? (
              <p id={`${ids}-thr-err`} className="text-xs text-destructive">
                {thrErr ?? errView?.problem}
              </p>
            ) : null}
          </FieldSet>

          <FieldSet className="gap-2">
            <FieldLegend variant="label" className="mb-0">
              {copy.at100}
            </FieldLegend>
            <RadioGroup
              aria-label={copy.at100}
              value={action}
              onValueChange={(a) => chooseAction(a as BudgetAction)}
              className="gap-2"
            >
              {(['alert', 'stop'] as const).map((a) => (
                <div key={a} className="flex items-start gap-2">
                  <RadioGroupItem
                    id={`${ids}-action-${a}`}
                    value={a}
                    className="mt-0.5"
                    aria-describedby={a === 'stop' && action === 'stop' ? `${ids}-stop` : undefined}
                  />
                  <Label
                    htmlFor={`${ids}-action-${a}`}
                    className="block leading-normal font-normal"
                  >
                    {a === 'alert' ? copy.actionAlert : copy.actionStop}
                    <span className="block text-xs text-muted-foreground">
                      {a === 'alert' ? copy.alertOnlyHint : copy.stopHint}
                    </span>
                  </Label>
                </div>
              ))}
            </RadioGroup>
            {action === 'stop' ? (
              <Warn id={`${ids}-stop`} className="ml-6" testId="stop-consequence">
                {scope === 'owner' || src?.scope === 'owner'
                  ? copy.stopConsequenceOwner(reset)
                  : copy.stopConsequenceAgent(label, reset)}
              </Warn>
            ) : null}
          </FieldSet>

          {src ? (
            confirmingDelete ? (
              <div className="space-y-2 rounded-md border border-destructive/40 p-3">
                <p ref={confirmRef} tabIndex={-1} className="font-medium outline-none">
                  {copy.deleteBudgetTitle(subject)}
                </p>
                <p className="text-xs text-muted-foreground">{copy.deleteBudgetBody}</p>
                <div className="flex gap-2">
                  <Button
                    type="button"
                    size="sm"
                    variant="destructive"
                    className="pointer-coarse:min-h-11"
                    disabled={busy}
                    onClick={remove}
                  >
                    {copy.deleteBudget}
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="ghost"
                    className="pointer-coarse:min-h-11"
                    onClick={() => {
                      setReturnToRemove(true)
                      setConfirmingDelete(false)
                    }}
                  >
                    {copy.cancel}
                  </Button>
                </div>
              </div>
            ) : (
              <LinkButton
                ref={removeRef}
                className="font-normal text-destructive"
                onClick={() => setConfirmingDelete(true)}
              >
                {copy.deleteBudget}
              </LinkButton>
            )
          ) : null}
        </div>
        <SheetFooter className="sticky bottom-0 border-t border-border bg-background">
          {errView ? (
            <p role="alert" className="text-sm text-destructive">
              {errView.problem} {errView.action}
            </p>
          ) : null}
          <div className="flex items-center justify-between gap-2">
            <p className="text-xs text-muted-foreground">{copy.budgetTiming}</p>
            <div className="flex gap-2">
              <Button
                type="button"
                variant="ghost"
                className="pointer-coarse:min-h-11"
                onClick={onClose}
              >
                {copy.cancel}
              </Button>
              <Button
                type="submit"
                className="pointer-coarse:min-h-11"
                disabled={(touched && invalid) || noScope || busy}
              >
                {save.isPending ? copy.saving : copy.save}
              </Button>
            </div>
          </div>
        </SheetFooter>
      </form>
    </>
  )
}
