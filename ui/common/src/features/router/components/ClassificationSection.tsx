/**
 * Request classification ([classifier] companion): the effective Level 3 backend as the server reports
 * it, and a side-effect-free preview of one query through the router's own classifier next to the regex
 * baseline. Classification only: it never shows a model or tier, because resolving one would mean the
 * routing policy and the preview must not touch routing state. Previews are superuser-only (a hosted
 * preview is a paid call) and only ever run on the button, never per keystroke.
 */
import { AlertTriangle, Lock } from 'lucide-react'
import { useId, useRef, useState } from 'react'
import { StateCard } from '@/components/shared/state-card'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'
import { useAnnounce } from '../announce'
import { useClassifierPreview, useClassifierStatus } from '../api'
import { copy } from '../copy'
import type {
  ClassifierPreview,
  ClassifierPreviewBackend,
  ClassifierPreviewResult,
  ClassifierStatus,
} from '../types'
import { Warn } from './bits'
import { SectionError } from './SectionError'

/** Matches the server's `MAX_PREVIEW_QUERY_CHARS`; the server rejects longer input anyway. */
const MAX_QUERY_CHARS = 8000

const backendName = (id: string) => copy.classifierBackendNames[id] ?? id
const typeName = (id: string) => copy.classifierTypes[id] ?? id
const ms = (us: number) => (us / 1000).toFixed(us < 10_000 ? 2 : 1)

export function ClassificationSection({
  superuser,
  active,
}: {
  superuser: boolean
  /** The tab is shown; status is read lazily so an unused tab costs nothing. */
  active: boolean
}) {
  const status = useClassifierStatus(active)
  return (
    <section
      id="router-classification"
      aria-labelledby="router-classification-h"
      className="scroll-mt-4 space-y-3"
    >
      <div className="space-y-0.5">
        <h2 id="router-classification-h" tabIndex={-1} className="text-sm font-semibold">
          {copy.classifierTitle}
        </h2>
        <p className="text-xs text-muted-foreground">{copy.classifierIntro}</p>
      </div>
      {status.isPending ? (
        <Skeleton aria-hidden className="h-24 w-full rounded-lg" />
      ) : status.isError ? (
        <SectionError error={status.error} onRetry={() => status.refetch()} />
      ) : (
        <>
          <StatusPanel status={status.data} />
          <PreviewForm status={status.data} superuser={superuser} />
        </>
      )}
    </section>
  )
}

function StatusPanel({ status }: { status: ClassifierStatus }) {
  const configured = backendName(status.configured_backend)
  const effective = backendName(status.effective_backend)
  const experimental = status.configured_backend !== 'regex'
  const local = status.configured_backend === 'laya'
  const degraded = status.effective_backend !== status.configured_backend
  const others = ['jev', 'laya']
    .filter((b) => b !== status.configured_backend)
    .map(backendName)
    .join(', ')
  return (
    <div className="space-y-2 rounded-lg border border-border bg-card p-4 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-semibold">{copy.classifierStatusTitle}</h3>
        <Badge variant={experimental ? 'secondary' : 'outline'}>
          {local
            ? copy.classifierLayaExperimental
            : experimental
              ? copy.classifierJevExperimental
              : copy.classifierRegexDefault}
        </Badge>
        {local && !degraded ? <Badge variant="outline">{copy.classifierLayaReady}</Badge> : null}
      </div>
      <ul className="grid gap-x-6 gap-y-1 text-xs sm:grid-cols-2">
        <li>{copy.classifierConfigured(configured)}</li>
        <li className={cn(degraded && 'font-medium text-warning')}>
          {copy.classifierEffective(effective)}
        </li>
        {status.model ? <li>{copy.classifierModel(status.model)}</li> : null}
        {status.endpoint_host ? <li>{copy.classifierEndpoint(status.endpoint_host)}</li> : null}
        {status.model_path ? (
          <li className="break-all">{copy.classifierModelPath(status.model_path)}</li>
        ) : null}
        <li>{copy.classifierTimeout(status.timeout_ms)}</li>
        <li>{copy.classifierFloor(status.min_confidence)}</li>
        {status.routing_seed_set ? <li>{copy.classifierSeed}</li> : null}
      </ul>
      <p className="text-xs text-muted-foreground tabular-nums">
        {copy.classifierStats(
          status.stats.calls,
          status.stats.fallback_total,
          status.stats.abstained,
        )}{' '}
        {copy.classifierStatsNote}
      </p>
      {status.init_error ? (
        <div className="space-y-1">
          <Warn testId="classifier-init-error">
            {copy.classifierUnavailable} {status.init_error}
          </Warn>
          <Setup backend={status.configured_backend} />
        </div>
      ) : !experimental ? (
        <Setup backend="regex" />
      ) : null}
      <p className="text-xs text-muted-foreground">{copy.classifierOtherBackends(others)}</p>
    </div>
  )
}

/** Setup guidance for the backend that is configured but unusable, or for both when on regex. */
function Setup({ backend }: { backend: string }) {
  const jev = (
    <>
      <span className="font-medium text-foreground">{copy.classifierSetupTitle}:</span>{' '}
      {copy.classifierSetup}
    </>
  )
  const laya = (
    <>
      <span className="font-medium text-foreground">{copy.classifierLayaSetupTitle}:</span>{' '}
      {copy.classifierLayaSetup}
    </>
  )
  return (
    <p className="text-xs text-muted-foreground">
      {backend === 'laya' ? laya : backend === 'jev' ? jev : null}
      {backend === 'regex' ? (
        <>
          {jev} {laya} {copy.classifierSetupBoth}
        </>
      ) : null}{' '}
      {copy.classifierSetupDocs}
    </p>
  )
}

function PreviewForm({ status, superuser }: { status: ClassifierStatus; superuser: boolean }) {
  const ids = useId()
  const announce = useAnnounce()
  const preview = useClassifierPreview()
  const [query, setQuery] = useState('')
  const [context, setContext] = useState('')
  const [backend, setBackend] = useState<ClassifierPreviewBackend>('configured')
  const [fieldError, setFieldError] = useState<string | null>(null)
  const [result, setResult] = useState<ClassifierPreview | null>(null)
  const [error, setError] = useState<unknown>(null)
  // The latest submission wins: a slower earlier response must never overwrite a newer one.
  const seq = useRef(0)
  const last = useRef<{ query: string; context: string; backend: ClassifierPreviewBackend } | null>(
    null,
  )

  const hosted = status.configured_backend !== 'regex'
  const configuredName = backendName(status.configured_backend)
  const allowed = superuser && status.preview_allowed

  const run = async (
    input: { query: string; context: string; backend: ClassifierPreviewBackend } | null,
  ) => {
    if (!input) return
    const mine = ++seq.current
    setError(null)
    try {
      const data = await preview.mutateAsync({
        query: input.query,
        context: input.context.trim() ? input.context : null,
        backend: input.backend,
      })
      if (mine !== seq.current) return
      setResult(data)
      announce(copy.classifierAnnounce(typeName(data.result.request_type), data.result.complexity))
    } catch (e) {
      if (mine !== seq.current) return
      setResult(null)
      setError(e)
    }
  }

  const submit = () => {
    const q = query.trim()
    if (!q) {
      setFieldError(copy.classifierQueryRequired)
      return
    }
    if (q.length > MAX_QUERY_CHARS) {
      setFieldError(copy.classifierQueryTooLong(MAX_QUERY_CHARS))
      return
    }
    setFieldError(null)
    last.current = { query: q, context, backend }
    void run(last.current)
  }

  return (
    <div className="space-y-3 rounded-lg border border-border bg-card p-4 text-sm">
      <h3 className="text-sm font-semibold">{copy.classifierFormTitle}</h3>
      {!allowed ? (
        <StateCard tone="info" title={copy.classifierNoRights} className="p-4">
          {copy.classifierNoRightsStatus}
        </StateCard>
      ) : null}
      <form
        className="space-y-3"
        aria-busy={preview.isPending || undefined}
        onSubmit={(e) => {
          e.preventDefault()
          submit()
        }}
      >
        <Field>
          <FieldLabel htmlFor={`${ids}-query`}>{copy.classifierQuery}</FieldLabel>
          <Textarea
            id={`${ids}-query`}
            value={query}
            rows={3}
            disabled={!allowed}
            aria-invalid={fieldError ? true : undefined}
            aria-describedby={fieldError ? `${ids}-query-error` : `${ids}-query-hint`}
            onChange={(e) => {
              setQuery(e.target.value)
              if (fieldError) setFieldError(null)
            }}
          />
          {fieldError ? (
            <p id={`${ids}-query-error`} className="text-xs text-destructive">
              {fieldError}
            </p>
          ) : (
            <FieldDescription id={`${ids}-query-hint`}>{copy.classifierQueryHint}</FieldDescription>
          )}
        </Field>
        <Field>
          <FieldLabel htmlFor={`${ids}-context`}>{copy.classifierContext}</FieldLabel>
          <Textarea
            id={`${ids}-context`}
            value={context}
            rows={2}
            disabled={!allowed}
            aria-describedby={`${ids}-context-hint`}
            onChange={(e) => setContext(e.target.value)}
          />
          <FieldDescription id={`${ids}-context-hint`}>
            {copy.classifierContextHint}
          </FieldDescription>
        </Field>
        <div className="flex flex-wrap items-end gap-3">
          {hosted ? (
            <Field className="min-w-56">
              <FieldLabel htmlFor={`${ids}-backend`}>{copy.classifierBackendPick}</FieldLabel>
              <Select
                value={backend}
                onValueChange={(v) => setBackend(v as ClassifierPreviewBackend)}
                disabled={!allowed}
              >
                <SelectTrigger id={`${ids}-backend`} className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="configured">
                    {copy.classifierPickConfigured(configuredName)}
                  </SelectItem>
                  <SelectItem value="regex">{copy.classifierPickRegex}</SelectItem>
                </SelectContent>
              </Select>
            </Field>
          ) : null}
          <Button
            type="submit"
            disabled={!allowed || preview.isPending}
            className="pointer-coarse:min-h-11"
          >
            {preview.isPending ? copy.classifierTesting : copy.classifierTest}
          </Button>
        </div>
      </form>
      <p className="text-xs text-muted-foreground">{copy.classifierPreviewOnly}</p>
      {error ? <SectionError error={error} onRetry={() => void run(last.current)} /> : null}
      {result ? <Results data={result} /> : null}
    </div>
  )
}

function Results({ data }: { data: ClassifierPreview }) {
  const configuredName = backendName(data.configured_backend)
  const single = data.backend === 'regex' || data.configured_backend === 'regex'
  return (
    <div
      role="region"
      aria-label={copy.classifierResultsLabel}
      className={cn('grid gap-3', !single && 'sm:grid-cols-2')}
    >
      {single ? (
        <ResultCard title={copy.classifierResultBaseline} result={data.baseline} />
      ) : (
        <>
          <ResultCard
            title={copy.classifierResultConfigured(configuredName)}
            result={data.result}
            initError={data.init_error}
          />
          <ResultCard title={copy.classifierResultBaseline} result={data.baseline} />
          <p className="text-xs text-muted-foreground sm:col-span-2">{copy.classifierResultSame}</p>
        </>
      )}
    </div>
  )
}

function ResultCard({
  title,
  result,
  initError,
}: {
  title: string
  result: ClassifierPreviewResult
  initError?: string | null
}) {
  const hostedAnswer = result.disposition === 'primary' || result.disposition === 'abstained'
  const local = result.answered_by === 'laya'
  const fellBack = result.disposition === 'fallback'
  const d = result.diagnostics
  return (
    <article
      aria-label={title}
      className="space-y-2 rounded-md border border-border p-3 text-sm"
      data-testid={`classifier-result-${hostedAnswer ? 'hosted' : 'regex'}`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <h4 className="font-medium">{title}</h4>
        <Badge variant="outline">
          {copy.classifierAnsweredBy(backendName(result.answered_by))}
        </Badge>
      </div>
      {fellBack ? (
        <p className="flex items-start gap-1.5 text-xs text-warning">
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
          <span>
            {copy.classifierFellBack(
              copy.classifierFallbackReasons[result.fallback_reason ?? ''] ??
                result.fallback_reason ??
                '',
            )}
            {initError ? ` (${initError})` : null}
          </span>
        </p>
      ) : null}
      {result.disposition === 'abstained' ? (
        <p className="flex items-start gap-1.5 text-xs text-warning">
          <Lock className="mt-0.5 size-3.5 shrink-0" aria-hidden />
          <span>{copy.classifierAbstained}</span>
        </p>
      ) : null}
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
        <dt className="text-muted-foreground">{copy.classifierType}</dt>
        <dd className="font-medium">{typeName(result.request_type)}</dd>
        <dt className="text-muted-foreground">{copy.classifierDifficulty}</dt>
        <dd className="tabular-nums">{copy.classifierDifficultyOf(result.complexity)}</dd>
        <dt className="text-muted-foreground">{copy.classifierConfidence}</dt>
        <dd className="tabular-nums">
          {hostedAnswer
            ? copy.classifierConfidenceHosted(Math.round(result.confidence * 100))
            : copy.classifierConfidenceRegex(result.confidence.toFixed(2))}
        </dd>
        <dt className="text-muted-foreground">{copy.classifierLatency}</dt>
        <dd className="tabular-nums">{copy.classifierLatencyValue(ms(result.latency_us))}</dd>
        <dt className="text-muted-foreground">{copy.classifierCost}</dt>
        <dd className="tabular-nums">
          {hostedAnswer
            ? d?.input_tokens != null
              ? local
                ? copy.classifierCostLocal(d.input_tokens)
                : copy.classifierCostTokens(d.input_tokens)
              : copy.classifierCostUnavailable
            : copy.classifierCostNone}
        </dd>
      </dl>
      {d?.model_version ? (
        <p className="text-xs text-muted-foreground">
          {copy.classifierModelVersion(d.model_version)}
        </p>
      ) : null}
      {result.input_truncated ? (
        <Warn>{local ? copy.classifierTruncatedWindow : copy.classifierTruncated}</Warn>
      ) : null}
    </article>
  )
}
