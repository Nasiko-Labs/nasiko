/**
 * Settings (plan §7.3, managers only): display name and description, feature flags (prompt comments, token
 * optimization; minimal-code and self-review for agents whose card reads as code work), secrets (names only;
 * values are write-only and cleared after submit), and delete (type the unique name).
 * Both forms are react-hook-form + zod (plan §2.4) and ask before a route change drops edits; switches save at once.
 */
import { zodResolver } from '@hookform/resolvers/zod'
import { useEffect, useId, useState, type ReactNode } from 'react'
import { useForm } from 'react-hook-form'
import { z } from 'zod'
import { Button } from '@/components/ui/button'
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldGroup,
  FieldLabel,
  FieldTitle,
} from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { LeaveGuard } from '@/components/shared/leave-guard'
import { useSecretMutations, useSecrets, useUpdateAgent } from '../api'
import { BetaBadge } from '@/components/shared/beta-badge'
import { ErrorNote, LearnMore, Section } from '../components/bits'
import { DeleteAgentDialog } from '../components/dialogs'
import { copy } from '../copy'
import { withFeature, type AgentView } from '../normalize'
import { SAVED_NOTE_MS } from '../tuning'

export function SettingsTab({ agent }: { agent: AgentView }) {
  return (
    <div className="space-y-4">
      <DetailsForm agent={agent} />
      {agent.isHarness ? null : (
        <>
          <Features agent={agent} />
          <TokenOptimization agent={agent} />
          {agent.codingSkills ? <CodingBehavior agent={agent} /> : null}
          <Secrets id={agent.id} />
        </>
      )}
      <DangerZone agent={agent} />
    </div>
  )
}

function useSavedNote() {
  const [on, setOn] = useState(false)
  useEffect(() => {
    if (!on) return
    const t = setTimeout(() => setOn(false), SAVED_NOTE_MS)
    return () => clearTimeout(t)
  }, [on])
  return [on, () => setOn(true)] as const
}

const detailsSchema = z.object({
  display_name: z.string().trim().min(1).max(200),
  description: z.string().max(2000),
})

function DetailsForm({ agent }: { agent: AgentView }) {
  const update = useUpdateAgent(agent.id)
  const [saved, flash] = useSavedNote()
  const form = useForm({
    resolver: zodResolver(detailsSchema),
    mode: 'onChange',
    defaultValues: { display_name: agent.displayName, description: agent.description },
  })
  const { isDirty, isValid, isSubmitting, errors } = form.formState
  const nameId = useId()
  const descId = useId()
  // The saved values become the new baseline, so the form is clean (and the guard off) after a save.
  const onSubmit = form.handleSubmit((v) =>
    update.mutate(v, {
      onSuccess: () => {
        form.reset(v)
        flash()
      },
    }),
  )
  return (
    <Section title={copy.settings}>
      <form className="max-w-xl space-y-3" onSubmit={onSubmit}>
        <Field className="gap-1">
          <FieldLabel htmlFor={nameId}>{copy.displayName}</FieldLabel>
          <Input
            id={nameId}
            maxLength={200}
            required
            aria-invalid={!!errors.display_name}
            {...form.register('display_name')}
          />
        </Field>
        <Field className="gap-1">
          <FieldLabel htmlFor={descId}>{copy.description}</FieldLabel>
          <Textarea id={descId} rows={3} maxLength={2000} {...form.register('description')} />
        </Field>
        <div className="flex items-center gap-3">
          <Button type="submit" size="sm" disabled={!isDirty || !isValid || update.isPending}>
            {update.isPending ? `${copy.save}…` : copy.save}
          </Button>
          {saved ? (
            <span role="status" className="text-xs text-success">
              {copy.saved}
            </span>
          ) : null}
        </div>
        {update.isError ? <ErrorNote error={update.error} context="manage" /> : null}
      </form>
      <LeaveGuard when={isDirty && !isSubmitting} />
    </Section>
  )
}

const secretSchema = z.object({
  name: z.string().trim().min(1),
  value: z.string().min(1),
})

function Secrets({ id }: { id: string }) {
  const secrets = useSecrets(id, true)
  const m = useSecretMutations(id)
  const [setError, setSetError] = useState<Error | null>(null)
  const [saved, flash] = useSavedNote()
  const form = useForm({
    resolver: zodResolver(secretSchema),
    mode: 'onChange',
    defaultValues: { name: '', value: '' },
  })
  const { isDirty, isValid, isSubmitting } = form.formState
  const nameId = useId()
  const valueId = useId()
  const onSubmit = form.handleSubmit((v) => {
    // Clear the value on submit either way: it must not linger in the DOM or the form state.
    form.setValue('value', '', { shouldDirty: true, shouldValidate: true })
    setSetError(null)
    // Always reset(): it drops the value from the mutation's cached variables; the error is kept locally.
    m.set.mutate(v, {
      onSuccess: () => {
        form.reset()
        flash()
      },
      onError: setSetError,
      onSettled: () => m.set.reset(),
    })
  })
  return (
    <Section title={copy.secrets} action={<LearnMore href="secrets" />}>
      <p className="text-xs text-muted-foreground">{copy.secretsHowItWorks}</p>
      {secrets.isPending ? (
        <Skeleton className="h-10" />
      ) : secrets.isError ? (
        <ErrorNote error={secrets.error} onRetry={() => void secrets.refetch()} />
      ) : secrets.data.length ? (
        <ul className="divide-y divide-border text-sm">
          {secrets.data.map((s) => (
            <li key={s.name} className="flex items-center justify-between py-1.5">
              <code className="font-mono text-xs">{s.name}</code>
              <Button
                size="sm"
                variant="ghost"
                disabled={m.remove.isPending}
                aria-label={copy.removeSecret(s.name)}
                onClick={() => m.remove.mutate(s.name)}
              >
                {copy.remove}
              </Button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-sm text-muted-foreground">{copy.noSecrets}</p>
      )}
      <form className="flex flex-wrap items-end gap-2" onSubmit={onSubmit}>
        <Field className="w-48 gap-1">
          <FieldLabel htmlFor={nameId}>{copy.secretName}</FieldLabel>
          <Input
            id={nameId}
            className="font-mono"
            autoComplete="off"
            spellCheck={false}
            required
            {...form.register('name')}
          />
        </Field>
        <Field className="w-56 gap-1">
          <FieldLabel htmlFor={valueId}>{copy.secretValue}</FieldLabel>
          <Input
            id={valueId}
            type="password"
            autoComplete="new-password"
            required
            {...form.register('value')}
          />
        </Field>
        <Button type="submit" size="sm" disabled={!isValid || m.set.isPending}>
          {copy.addSecret}
        </Button>
        {saved ? (
          <span role="status" className="text-xs text-success">
            {copy.saved}
          </span>
        ) : null}
      </form>
      {setError ? <ErrorNote error={setError} context="secret" /> : null}
      {m.remove.isError ? <ErrorNote error={m.remove.error} context="secret" /> : null}
      <LeaveGuard when={isDirty && !isSubmitting} />
    </Section>
  )
}

/** One switch row: label and hint on the left, the switch on the right. It saves on change. */
function FlagRow({
  label,
  hint,
  checked,
  disabled,
  onCheckedChange,
}: {
  label: string
  hint: ReactNode
  checked: boolean
  disabled?: boolean
  onCheckedChange: (on: boolean) => void
}) {
  const id = useId()
  return (
    <Field orientation="horizontal" data-disabled={disabled} className="gap-6">
      <FieldContent>
        <FieldLabel htmlFor={id}>{label}</FieldLabel>
        <FieldDescription>{hint}</FieldDescription>
      </FieldContent>
      <Switch id={id} checked={checked} disabled={disabled} onCheckedChange={onCheckedChange} />
    </Field>
  )
}

/** While a write is in flight the switch shows where it is going (the mutation stays pending until the re-read). */
const shown = (saved: boolean, pending: boolean) => (pending ? !saved : saved)

function Features({ agent }: { agent: AgentView }) {
  const update = useUpdateAgent(agent.id)
  return (
    <Section title={copy.features} subtitle={copy.featuresHint} action={<BetaBadge />}>
      <FlagRow
        label={copy.promptComments}
        hint={
          <>
            {copy.promptCommentsHint}{' '}
            {/* No ligatures: JetBrains Mono draws `<!--` and `-->` as arrows. */}
            <code className="font-mono whitespace-nowrap [font-variant-ligatures:none]">
              {copy.promptCommentsOptOut}
            </code>
            .
          </>
        }
        checked={shown(agent.promptComments, update.isPending)}
        disabled={update.isPending}
        onCheckedChange={(on) =>
          update.mutate({ metadata: withFeature(agent.metadata, 'prompt_comments', on) })
        }
      />
      {update.isError ? <ErrorNote error={update.error} context="manage" /> : null}
    </Section>
  )
}

function TokenOptimization({ agent }: { agent: AgentView }) {
  const update = useUpdateAgent(agent.id)
  return (
    <Section
      title={copy.tokenOptimization}
      subtitle={copy.tokenOptimizationIntro}
      action={<BetaBadge />}
    >
      <FlagRow
        label={copy.tokenOptimization}
        hint={copy.tokenOptimizationHint}
        checked={shown(agent.compress, update.isPending)}
        disabled={update.isPending}
        onCheckedChange={(on) => update.mutate({ compress_enabled: on })}
      />
      {update.isError ? <ErrorNote error={update.error} context="manage" /> : null}
    </Section>
  )
}

/**
 * `CODING_AGENT_SELF_REVIEW` (nasiko-coding-policy `self_review_enabled`) is an agent secret, and unset reads as on.
 * So the switch writes "false" to turn it off and removes the secret to turn it on: the secret listed means off.
 *
 * That agent-side default makes self-review **opt-out**, which is the wrong shape for a switch that
 * costs an extra model turn on every edit. Turning the ladder on therefore also writes `false` when
 * no explicit choice has been recorded, so the extra turn is something you ask for rather than
 * something that starts happening because you enabled a different feature. The switch becomes
 * available at that moment, and reads off because it genuinely is off.
 *
 * Changing the default in `coding-policy` would have been the tidier fix, but it is read by
 * vendored copies inside each agent image (`vendor/coding-policy/`), so already-deployed agents
 * would keep the old default until rebuilt — the two would disagree, and the switch would lie.
 */
// ponytail: a value of "true" set from the CLI reads as off here; exact once the server stores it as a column like minimal_code_enabled.
const SELF_REVIEW_SECRET = 'CODING_AGENT_SELF_REVIEW'

function CodingBehavior({ agent }: { agent: AgentView }) {
  const minimal = useUpdateAgent(agent.id)
  const secrets = useSecrets(agent.id, true)
  const review = useSecretMutations(agent.id)
  const minimalOn = shown(agent.minimalCode, minimal.isPending)
  const reviewPending = review.set.isPending || review.remove.isPending
  const reviewSaved = secrets.isSuccess && !secrets.data.some((s) => s.name === SELF_REVIEW_SECRET)
  const reviewError = review.set.error ?? review.remove.error
  return (
    <Section title={copy.codingBehavior} action={<BetaBadge />}>
      <FieldGroup className="gap-5">
        <FlagRow
          label={copy.minimalCode}
          hint={copy.minimalCodeHint}
          checked={minimalOn}
          disabled={minimal.isPending || reviewPending}
          onCheckedChange={(on) => {
            minimal.mutate({ minimal_code_enabled: on })
            // `reviewSaved` means no secret is stored, which the agent reads as self-review ON.
            // Pin it off as the ladder goes on, so enabling one feature never silently enables a
            // second one that costs an extra model turn per edit.
            if (on && reviewSaved) {
              review.set.mutate({ name: SELF_REVIEW_SECRET, value: 'false' })
            }
          }}
        />
        {/* A child of Minimal-code mode: the agent only reviews when the ladder is on (wants_self_review), so it
            reads off and can't be changed while the parent is off. */}
        <div className="border-l border-border pl-4">
          <FlagRow
            label={copy.selfReview}
            hint={copy.selfReviewHint}
            checked={minimalOn && shown(reviewSaved, reviewPending)}
            disabled={!minimalOn || !secrets.isSuccess || reviewPending}
            onCheckedChange={(on) =>
              on
                ? review.remove.mutate(SELF_REVIEW_SECRET)
                : review.set.mutate({ name: SELF_REVIEW_SECRET, value: 'false' })
            }
          />
        </div>
      </FieldGroup>
      {minimal.isError ? <ErrorNote error={minimal.error} context="manage" /> : null}
      {reviewError ? <ErrorNote error={reviewError} context="secret" /> : null}
    </Section>
  )
}

function DangerZone({ agent }: { agent: AgentView }) {
  const [open, setOpen] = useState(false)
  return (
    <Section title={copy.dangerZone} className="border-destructive/40">
      <Field orientation="horizontal" className="flex-wrap justify-between gap-4">
        <FieldContent className="min-w-48">
          <FieldTitle>{copy.deleteThisAgent}</FieldTitle>
          <FieldDescription>{copy.deleteBody}</FieldDescription>
        </FieldContent>
        <Button
          variant="outline"
          size="sm"
          className="border-destructive/40 text-destructive hover:bg-destructive/10 hover:text-destructive"
          onClick={() => setOpen(true)}
        >
          {copy.deleteAgent}
        </Button>
      </Field>
      <DeleteAgentDialog agent={agent} open={open} onOpenChange={setOpen} />
    </Section>
  )
}
