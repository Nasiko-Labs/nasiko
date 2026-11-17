/**
 * The free TokenOps assessment (`/assessment`; the sidebar card that replaced the waitlist's): a hero, a short form
 * about how the visitor runs agents, then the last step: the sales Calendly embedded on the page, as Contact sales and
 * nasiko.com/demo do. Submitting POSTs the answers (api.ts, stored in Neon and sent to Clay) before the last step. When
 * the person books, Calendly tells the page (`calendly.event_scheduled`) and the page reports it
 * (`POST /api/assessments/{id}/booking`), which marks the saved assessment booked and sends it to Clay. A failed save —
 * a server without an intake configured, or the intake down — still reaches the last step, which then asks for the
 * answers in the booking note instead.
 */
import { Check, ChartNoAxesColumn, MessagesSquare, Waypoints } from 'lucide-react'
import { useCallback, useEffect, useId, useRef, useState } from 'react'
import { Controller, useForm, useWatch, type RegisterOptions } from 'react-hook-form'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { SpotlightCard } from '@nasiko/ui/composites/spotlight-card'
import { CalendlyEmbed, type CalendlyBooking } from '@nasiko/ui/composites/calendly-embed'
import { CopyButton } from '@nasiko/ui/composites/copy-button'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import { Card } from '@nasiko/ui/primitives/card'
import { Checkbox } from '@nasiko/ui/primitives/checkbox'
import { Field, FieldError, FieldLabel, FieldLegend, FieldSet } from '@nasiko/ui/primitives/field'
import { Input } from '@nasiko/ui/primitives/input'
import { Label } from '@nasiko/ui/primitives/label'
import { RadioGroup, RadioGroupItem } from '@nasiko/ui/primitives/radio-group'
import { Textarea } from '@nasiko/ui/primitives/textarea'
import { SALES_CALL_URL } from '@nasiko/app-core/links'
import { useReportAssessmentBooking, useSubmitAssessment } from './api'
import { AGENT_COUNTS, APPLYING_AS, CHALLENGES, copy, SPEND } from './copy'

export type Answers = {
  applyingAs: (typeof APPLYING_AS)[number]['value']
  name: string
  email: string
  company: string
  agents: string
  spend: string
  challenges: string[]
  notes: string
}

const POINT_ICONS = [ChartNoAxesColumn, Waypoints, MessagesSquare]

/** The answers as plain text, for the booking note. */
function summaryOf(a: Answers): string {
  const label = (opts: readonly { value: string; label: string }[], v: string) =>
    opts.find((o) => o.value === v)?.label ?? v
  return [
    `Applying as: ${label(APPLYING_AS, a.applyingAs)}`,
    `Name: ${a.name}`,
    `Email: ${a.email}`,
    a.applyingAs === 'team' ? `Company: ${a.company}` : null,
    `Agents: ${label(AGENT_COUNTS, a.agents)}`,
    `Monthly token spend: ${label(SPEND, a.spend)}`,
    a.challenges.length ? `Challenges: ${a.challenges.join('; ')}` : null,
    a.notes.trim() ? `Setup / blocker: ${a.notes.trim()}` : null,
  ]
    .filter(Boolean)
    .join('\n')
}

/** The Calendly embed's URL: inline, prefilled, and tagged with the saved assessment when there is one. */
function bookingUrl(a: Answers, savedId: string | null): string {
  const url = new URL(SALES_CALL_URL)
  // What Calendly's own embed script adds: without them Calendly doesn't post its events to this page.
  url.searchParams.set('embed_domain', window.location.host)
  url.searchParams.set('embed_type', 'Inline')
  url.searchParams.set('hide_gdpr_banner', '1')
  url.searchParams.set('name', a.name.trim())
  url.searchParams.set('email', a.email.trim())
  // The event's first custom question is Company Name (`a1`); an individual has none, so types it there if they like.
  if (a.applyingAs === 'team') url.searchParams.set('a1', a.company.trim())
  url.searchParams.set('utm_source', 'nasiko-app')
  url.searchParams.set('utm_medium', 'tokenops-assessment')
  if (savedId) url.searchParams.set('utm_content', savedId)
  return url.toString()
}

const EMPTY: Answers = {
  applyingAs: 'individual',
  name: '',
  email: '',
  company: '',
  agents: '',
  spend: '',
  challenges: [],
  notes: '',
}

export function AssessmentPage() {
  // The last answers: shown on the last step, and the form's starting values after Edit answers.
  const [answers, setAnswers] = useState<Answers>(EMPTY)
  const [submitted, setSubmitted] = useState(false)
  // The stored assessment's id, or null when the save failed.
  const [savedId, setSavedId] = useState<string | null>(null)
  const save = useSubmitAssessment()
  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6">
      <TopBar root={copy.title} />
      <SpotlightCard>
        <div className="relative flex flex-col gap-4 p-6 sm:p-8">
          <Badge variant="secondary" className="self-start font-mono tracking-[0.04em] uppercase">
            {copy.badge}
          </Badge>
          <h1 className="text-2xl font-semibold text-balance sm:text-3xl">{copy.headline}</h1>
          <p className="text-sm text-muted-foreground sm:text-base">{copy.lead}</p>
          <ul className="flex flex-col gap-2 text-sm">
            {copy.points.map((p, i) => {
              const Icon = POINT_ICONS[i] ?? Check
              return (
                <li key={p} className="flex items-center gap-2.5">
                  <Icon aria-hidden className="size-4 text-primary-text" />
                  {p}
                </li>
              )
            })}
          </ul>
        </div>
      </SpotlightCard>
      {submitted ? (
        <LastStep answers={answers} savedId={savedId} onEdit={() => setSubmitted(false)} />
      ) : (
        <AssessmentForm
          initial={answers}
          sending={save.isPending}
          onDone={async (a) => {
            setAnswers(a)
            setSavedId(
              await save.mutateAsync(a).then(
                (r) => r.id,
                () => null,
              ),
            )
            setSubmitted(true)
          }}
        />
      )}
    </div>
  )
}

// useForm lives here, not in the page: under the React Compiler a child reading a passed-in form's formState never
// sees its errors update.
function AssessmentForm({
  initial,
  sending,
  onDone,
}: {
  initial: Answers
  sending: boolean
  onDone(a: Answers): Promise<void>
}) {
  const id = useId()
  const form = useForm<Answers>({ defaultValues: initial })
  const { errors } = form.formState
  const team = useWatch({ control: form.control, name: 'applyingAs' }) === 'team'
  const submit = form.handleSubmit(onDone)
  const req = <span className="sr-only">{copy.form.required}</span>
  const star = (
    <span aria-hidden className="text-muted-foreground">
      *
    </span>
  )

  const choices = (
    name: 'applyingAs' | 'agents' | 'spend',
    legend: string,
    options: readonly { value: string; label: string }[],
    message?: string,
    hint?: string,
  ) => (
    <FieldSet data-invalid={!!errors[name]}>
      <FieldLegend variant="label" id={`${id}-${name}`} className="mb-1">
        {legend} {message ? star : null}
        {message ? req : null}
      </FieldLegend>
      {hint ? <p className="-mt-1 text-xs text-muted-foreground">{hint}</p> : null}
      <Controller
        control={form.control}
        name={name}
        rules={message ? { required: message } : undefined}
        render={({ field }) => (
          <RadioGroup
            ref={field.ref}
            aria-labelledby={`${id}-${name}`}
            aria-invalid={!!errors[name]}
            value={field.value}
            onValueChange={field.onChange}
            className="gap-2.5"
          >
            {options.map((o) => (
              <Label key={o.value} className="flex items-center gap-3 font-normal">
                <RadioGroupItem value={o.value} />
                {o.label}
              </Label>
            ))}
          </RadioGroup>
        )}
      />
      <FieldError errors={[errors[name]]} />
    </FieldSet>
  )

  const text = (
    name: 'name' | 'email' | 'company',
    label: string,
    placeholder: string,
    rules: RegisterOptions<Answers, 'name' | 'email' | 'company'>,
    type = 'text',
    autoComplete?: string,
  ) => (
    <Field data-invalid={!!errors[name]}>
      <FieldLabel htmlFor={`${id}-${name}`}>
        {label} {star}
        {req}
      </FieldLabel>
      <Input
        id={`${id}-${name}`}
        type={type}
        autoComplete={autoComplete}
        placeholder={placeholder}
        aria-invalid={!!errors[name]}
        {...form.register(name, rules)}
      />
      <FieldError errors={[errors[name]]} />
    </Field>
  )

  return (
    <Card className="gap-0 p-6 sm:p-8">
      <h2 className="text-lg font-semibold">{copy.form.title}</h2>
      <p className="mt-1 text-sm text-muted-foreground">{copy.form.sub}</p>
      <form noValidate onSubmit={(e) => void submit(e)} className="mt-6 flex flex-col gap-7">
        {choices('applyingAs', copy.form.applyingAs, APPLYING_AS)}
        {text(
          'name',
          copy.form.name,
          copy.form.namePlaceholder,
          {
            validate: (v: string) => !!v.trim() || copy.errors.name,
          },
          'text',
          'name',
        )}
        {text(
          'email',
          copy.form.email,
          copy.form.emailPlaceholder,
          {
            required: copy.errors.email,
            pattern: { value: /^[^\s@]+@[^\s@]+\.[^\s@]+$/, message: copy.errors.emailFormat },
          },
          'email',
          'email',
        )}
        {/* Unregistered when hidden, so an individual's submit never waits on it. */}
        {team
          ? text(
              'company',
              copy.form.company,
              copy.form.companyPlaceholder,
              {
                validate: (v: string) => !!v.trim() || copy.errors.company,
                shouldUnregister: true,
              },
              'text',
              'organization',
            )
          : null}
        {choices('agents', copy.form.agents, AGENT_COUNTS, copy.errors.agents)}
        {choices('spend', copy.form.spend, SPEND, copy.errors.spend, copy.form.spendHint)}
        <FieldSet>
          <FieldLegend variant="label" className="mb-1">
            {copy.form.challenges}
          </FieldLegend>
          <p className="-mt-1 text-xs text-muted-foreground">{copy.form.challengesHint}</p>
          <Controller
            control={form.control}
            name="challenges"
            render={({ field }) => (
              <div className="flex flex-col gap-2.5">
                {CHALLENGES.map((c) => (
                  <Label key={c} className="flex items-center gap-3 font-normal">
                    <Checkbox
                      checked={field.value.includes(c)}
                      onCheckedChange={(on) =>
                        field.onChange(
                          on ? [...field.value, c] : field.value.filter((v) => v !== c),
                        )
                      }
                    />
                    {c}
                  </Label>
                ))}
              </div>
            )}
          />
        </FieldSet>
        <Field>
          <FieldLabel htmlFor={`${id}-notes`}>{copy.form.notes}</FieldLabel>
          <Textarea
            id={`${id}-notes`}
            rows={4}
            placeholder={copy.form.notesPlaceholder}
            {...form.register('notes')}
          />
        </Field>
        <div className="flex flex-col gap-3">
          <Button type="submit" size="lg" className="w-full" disabled={sending}>
            {sending ? copy.form.sending : copy.form.submit}
          </Button>
          <p className="text-center text-xs text-muted-foreground">{copy.form.fine}</p>
        </div>
      </form>
    </Card>
  )
}

function LastStep({
  answers,
  savedId,
  onEdit,
}: {
  answers: Answers
  savedId: string | null
  onEdit(): void
}) {
  const [booked, setBooked] = useState(false)
  const { mutate } = useReportAssessmentBooking()
  const onScheduled = useCallback(
    (booking: CalendlyBooking) => {
      setBooked(true)
      if (savedId) mutate({ id: savedId, booking })
    },
    [savedId, mutate],
  )
  const first = answers.name.trim().split(/\s+/)[0] ?? ''
  const heading = useRef<HTMLHeadingElement>(null)
  // Focus lands on the heading when the step swaps in (and again once booked), so a screen reader hears where it is.
  useEffect(() => heading.current?.focus(), [booked])
  if (booked) {
    return (
      <Card className="gap-4 p-6 sm:p-8">
        <h2 ref={heading} tabIndex={-1} className="text-lg font-semibold outline-none">
          {copy.booked.title}
        </h2>
        <p className="text-sm text-muted-foreground">{copy.booked.body(answers.email.trim())}</p>
      </Card>
    )
  }
  const summary = summaryOf(answers)
  return (
    <Card className="gap-4 p-6 sm:p-8">
      <h2 ref={heading} tabIndex={-1} className="text-lg font-semibold outline-none">
        {copy.done.title(first)}
      </h2>
      <p className="text-sm text-muted-foreground">
        {savedId ? copy.done.saved : copy.done.notSaved} {copy.done.line}
      </p>
      <CalendlyEmbed
        src={bookingUrl(answers, savedId)}
        title={copy.done.frameTitle}
        loadingLabel={copy.done.loading}
        onScheduled={onScheduled}
      />
      {savedId ? null : (
        <div className="mt-2 flex flex-col gap-2 rounded-lg border bg-muted/40 p-4">
          <div className="flex items-center justify-between gap-2">
            <h3 className="text-sm font-semibold">{copy.done.summary}</h3>
            <CopyButton text={summary} label={copy.done.copyAnswers} />
          </div>
          <p className="text-xs text-muted-foreground">{copy.done.pasteHint}</p>
          <pre className="font-mono text-xs break-words whitespace-pre-wrap">{summary}</pre>
        </div>
      )}
      <Button variant="ghost" size="sm" className="self-start" onClick={onEdit}>
        {copy.done.edit}
      </Button>
    </Card>
  )
}
