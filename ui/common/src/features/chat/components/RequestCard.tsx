/**
 * A request that pauses a turn (plan §6.8, EN5, DS7). Buttons render only while the request is
 * pending: `allowed_actions` is static per kind (hitl.rs:256-262), so it can't tell a receipt
 * from a live request. Tool approvals carry no arguments on this server (protocol.rs:700-730).
 * The typed answer is react-hook-form + zod (plan §2.4). It has no leave guard: the card sits in a
 * live turn, not a page form, and leaving the chat keeps the request pending to answer later.
 */
import { CircleAlert, CircleCheck, KeyRound, ShieldQuestion } from 'lucide-react'
import { zodResolver } from '@hookform/resolvers/zod'
import { useId, useState } from 'react'
import { Controller, useForm } from 'react-hook-form'
import { z } from 'zod'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Field, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field'
import { Textarea } from '@/components/ui/textarea'
import { AgentLink } from '@/features/agents/components/AgentLink'
import { safeHttpUrl } from '@/features/agents/normalize'
import { ApiError } from '@/lib/api/client'
import { fmtLongDay, fmtUtcTime } from '@/lib/format'
import type { ResolveBody } from '../api'
import { announce } from '../announce'
import { copy } from '../copy'
import type { HitlDto, HitlFrame, HitlOption } from '../types'

export interface RequestActions {
  resolve(id: string, body: ResolveBody): Promise<HitlDto>
  cancel(id: string): Promise<HitlDto>
  /** Called once a request is answered or dismissed, to start the resume. */
  onDone(req: HitlDto): void
}

export function RequestCard({
  request,
  frame,
  agentName,
  actions,
  id,
  label,
}: {
  request?: HitlDto
  /** The live stream's frame, used until history has the row. */
  frame?: HitlFrame
  agentName: string
  actions: RequestActions
  id?: string
  /** "Request i of N" when a pager shows one of several. */
  label?: string
}) {
  const kind = request?.kind ?? frame?.kind ?? 'input_required'
  const question = request?.question ?? frame?.question ?? null
  const status = request?.status ?? 'pending'
  const requestId = request?.id ?? frame?.id ?? ''
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [authStarted, setAuthStarted] = useState(false)
  const hintId = useId()

  if (status !== 'pending') {
    const Icon = status === 'resolved' ? CircleCheck : CircleAlert
    const answer =
      request?.human_response && typeof request.human_response === 'object'
        ? (request.human_response as { answer?: unknown }).answer
        : undefined
    return (
      <div
        className="flex items-start gap-2 rounded-md border border-border bg-muted/30 px-3 py-2 text-sm"
        data-testid="request-receipt"
      >
        <Icon className="mt-0.5 size-4 text-muted-foreground" aria-hidden />
        <div>
          <span className="font-medium">{copy.request.receipt[status] ?? status}</span>
          {question?.message ? (
            <span className="text-muted-foreground"> · {question.message}</span>
          ) : null}
          {answer !== undefined ? (
            <div className="text-muted-foreground">
              “{Array.isArray(answer) ? answer.join(', ') : String(answer)}”
            </div>
          ) : null}
        </div>
      </div>
    )
  }

  // Shown on the card and spoken by the page's announcer (the only live region, §8.1).
  const fail = (err: unknown) => {
    const text =
      err instanceof ApiError && err.serverMessage
        ? copy.serverSaid(err.serverMessage)
        : copy.request.submitFailed
    setError(text)
    announce(text)
  }

  // Promise chains, not try/finally: the React Compiler can't compile a finally clause yet.
  const run = (label: string, fn: () => Promise<HitlDto>) => {
    setBusy(label)
    setError(null)
    return new Promise<HitlDto>((ok) => ok(fn()))
      .then((res) => {
        if (res.already_resolved || res.already_canceled) {
          setNote(copy.request.handledElsewhere)
          announce(copy.request.handledElsewhere)
        }
        actions.onDone(res)
      })
      .catch((err: unknown) => {
        if (err instanceof ApiError && (err.status === 409 || err.status === 404)) {
          setNote(copy.request.handledElsewhere)
          announce(copy.request.handledElsewhere)
          actions.onDone({ ...(request as HitlDto), id: requestId, status: 'expired' })
        } else fail(err)
      })
      .finally(() => setBusy(null))
  }
  const resolve = (label: string, body: ResolveBody) =>
    run(label, () => actions.resolve(requestId, body))

  const startAuth = () => {
    if (busy || authStarted) return
    setBusy('start')
    setError(null)
    return new Promise<unknown>((ok) => ok(actions.resolve(requestId, { auth_action: 'start' })))
      .then(() => setAuthStarted(true))
      .catch(fail)
      .finally(() => setBusy(null))
  }

  const options = Array.isArray(question?.options) ? question.options : []
  const multi = question?.multi_select === true
  const allowCustom =
    question?.allow_custom_input === true || (!options.length && kind === 'input_required')
  const tool = typeof question?.tool_name === 'string' ? question.tool_name : copy.request.thisTool
  const provider =
    typeof question?.provider === 'string' ? question.provider : copy.request.theService
  const agentId = request?.execution.agent_id ?? null
  const requestedBy = (
    <>
      {copy.request.requestedBy}{' '}
      <AgentLink id={agentId} name={agentName}>
        {agentName}
      </AgentLink>
    </>
  )
  const authUrl = safeHttpUrl(typeof question?.auth_url === 'string' ? question.auth_url : null)
  const canCancel = !request || request.allowed_actions.includes('cancel')
  const expires = request?.expires_at ? fmtLongDay(request.expires_at) : ''

  const Icon = kind === 'auth_required' ? KeyRound : ShieldQuestion
  return (
    <section
      id={id}
      tabIndex={-1}
      aria-label={label ? `${copy.request.header} · ${label}` : copy.request.header}
      className="rounded-lg border border-warning/40 bg-warning/5 p-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
      data-testid="request-card"
    >
      <div className="flex items-start gap-2">
        <Icon className="mt-0.5 size-4 text-warning" aria-hidden />
        <div className="min-w-0 flex-1 space-y-2">
          {kind === 'tool_approval' ? (
            <>
              <div className="font-mono font-medium">{tool}</div>
              <div className="text-xs text-muted-foreground">
                {requestedBy}
                {request?.created_at ? ` · ${fmtUtcTime(request.created_at)} UTC` : ''}
              </div>
              {/* quirk: §10.5 — tool approvals carry no arguments. */}
              <p className="text-xs text-muted-foreground">{copy.request.noArguments}</p>
            </>
          ) : (
            <>
              {question?.header ? <div className="font-medium">{question.header}</div> : null}
              <p>{question?.message ?? copy.request.header}</p>
              <div className="text-xs text-muted-foreground">{requestedBy}</div>
            </>
          )}

          {kind === 'input_required' && options.length && !multi ? (
            <div className="flex flex-wrap gap-2">
              {options.map((o) => (
                <Button
                  key={o.label}
                  size="sm"
                  variant="outline"
                  className="pointer-coarse:min-h-11"
                  disabled={!!busy}
                  onClick={() => void resolve(o.label, { answer: o.label })}
                  title={o.description}
                >
                  {o.label}
                </Button>
              ))}
            </div>
          ) : null}

          {kind === 'input_required' && (multi || allowCustom) ? (
            <AnswerForm
              multi={multi}
              allowCustom={allowCustom}
              options={options}
              legend={question?.message ?? copy.request.header}
              busy={busy}
              onAnswer={(body) => void resolve('submit', body)}
            />
          ) : null}

          {kind === 'tool_approval' ? (
            <div className="flex flex-wrap items-center gap-2">
              <Button
                size="sm"
                className="pointer-coarse:min-h-11"
                disabled={!!busy}
                onClick={() => void resolve('once', { decision: 'approve', scope: 'once' })}
              >
                {copy.request.approveOnce}
              </Button>
              <Button
                size="sm"
                variant="outline"
                className="pointer-coarse:min-h-11"
                disabled={!!busy}
                aria-describedby={hintId}
                onClick={() => void resolve('session', { decision: 'approve', scope: 'session' })}
              >
                {copy.request.alwaysAllow(tool)}
              </Button>
              <Button
                size="sm"
                variant="outline"
                className="pointer-coarse:min-h-11"
                disabled={!!busy}
                onClick={() => void resolve('reject', { decision: 'reject' })}
              >
                {copy.request.reject}
              </Button>
              <span id={hintId} className="w-full text-xs text-muted-foreground">
                {copy.request.alwaysAllowHint}
              </span>
            </div>
          ) : null}

          {kind === 'auth_required' ? (
            authStarted ? (
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-muted-foreground">{copy.request.waitingSignIn}</span>
                <Button
                  size="sm"
                  className="pointer-coarse:min-h-11"
                  disabled={!!busy}
                  onClick={() => void resolve('confirm', { auth_action: 'confirm' })}
                >
                  {copy.request.signedIn}
                </Button>
              </div>
            ) : authUrl ? (
              // A real link: the browser opens it inside the click, so popup blockers leave it alone.
              <div className="flex flex-wrap items-center gap-2">
                <Button asChild size="sm" className="pointer-coarse:min-h-11">
                  <a
                    href={authUrl}
                    target="_blank"
                    rel="noopener noreferrer"
                    onClick={() => void startAuth()}
                    onAuxClick={() => void startAuth()}
                  >
                    {copy.request.signIn(provider)}
                  </a>
                </Button>
                <span className="text-xs text-muted-foreground">
                  {copy.request.opensHost(new URL(authUrl).host)}
                </span>
              </div>
            ) : (
              <Button
                size="sm"
                className="pointer-coarse:min-h-11"
                disabled={!!busy}
                onClick={() => void startAuth()}
              >
                {copy.request.signIn(provider)}
              </Button>
            )
          ) : null}

          {canCancel ? (
            <div className="flex flex-wrap items-center gap-2 pt-1">
              <Button
                size="xs"
                variant="ghost"
                className="min-h-8 pointer-coarse:min-h-11"
                disabled={!!busy}
                onClick={() => void run('cancel', () => actions.cancel(requestId))}
              >
                {copy.request.dismiss}
              </Button>
              <span className="text-xs text-muted-foreground">{copy.request.dismissHint}</span>
            </div>
          ) : (
            // quirk: every kind allows cancel at cb3aaf0c; kept for a server that drops it.
            <p className="text-xs text-muted-foreground">{copy.request.noCancel(expires)}</p>
          )}
          {error ? <p className="text-xs text-destructive">{error}</p> : null}
          {note ? <p className="text-xs text-muted-foreground">{note}</p> : null}
        </div>
      </div>
    </section>
  )
}

/** A typed or multi-select answer: some text, or (multi-select) at least one option or some text. */
const answerSchema = z
  .object({ multi: z.boolean(), picked: z.array(z.string()), custom: z.string() })
  .refine((v) => !!v.custom.trim() || (v.multi && v.picked.length > 0))
type Answer = z.infer<typeof answerSchema>

function AnswerForm({
  multi,
  allowCustom,
  options,
  legend,
  busy,
  onAnswer,
}: {
  multi: boolean
  allowCustom: boolean
  options: HitlOption[]
  legend: string
  busy: string | null
  onAnswer(body: ResolveBody): void
}) {
  const base = useId()
  const {
    control,
    register,
    handleSubmit,
    formState: { isValid },
  } = useForm<Answer>({
    resolver: zodResolver(answerSchema),
    mode: 'onChange',
    defaultValues: { multi, picked: [], custom: '' },
  })
  const submit = handleSubmit(({ picked, custom }) => {
    const text = custom.trim()
    onAnswer(
      multi ? { answer: picked, ...(text ? { custom_answer: text } : {}) } : { answer: text },
    )
  })
  return (
    <form className="space-y-2" onSubmit={(e) => void submit(e)}>
      {multi ? (
        <FieldSet className="gap-1">
          <FieldLegend className="sr-only">{legend}</FieldLegend>
          <Controller
            control={control}
            name="picked"
            render={({ field }) => (
              <>
                {options.map((o, i) => (
                  <Field
                    key={o.label}
                    orientation="horizontal"
                    className="gap-2 pointer-coarse:min-h-11"
                  >
                    <Checkbox
                      id={`${base}-${i}`}
                      checked={field.value.includes(o.label)}
                      onCheckedChange={(on) =>
                        field.onChange(
                          on === true
                            ? [...field.value, o.label]
                            : field.value.filter((x) => x !== o.label),
                        )
                      }
                    />
                    <FieldLabel htmlFor={`${base}-${i}`} className="font-normal">
                      {o.label}
                    </FieldLabel>
                  </Field>
                ))}
              </>
            )}
          />
        </FieldSet>
      ) : null}
      {allowCustom ? (
        <Field>
          <Textarea
            aria-label={options.length ? copy.request.somethingElse : legend}
            placeholder={options.length ? copy.request.somethingElse : ''}
            rows={2}
            {...register('custom')}
          />
        </Field>
      ) : null}
      <Button
        size="sm"
        type="submit"
        className="pointer-coarse:min-h-11"
        disabled={!!busy || !isValid}
      >
        {busy === 'submit' ? copy.loading : copy.request.submit}
      </Button>
    </form>
  )
}
