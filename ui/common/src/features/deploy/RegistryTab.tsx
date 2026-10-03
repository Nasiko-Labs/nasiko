/**
 * From a registry (plans/feat-deploy.md §4.3; design review 6, 7; eng review R4). One field; the import is a single
 * long request, so the progress is honest: one live step with elapsed time, then Running, never timer-driven steps.
 * It runs in the upload registry (uploads.ts), so leaving the page keeps it going.
 */
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { AlertTriangle, Dot } from 'lucide-react'
import { useEffect, useId, useState, useSyncExternalStore } from 'react'
import { StateCard } from '@/components/shared/state-card'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { sendRegistryImport } from './api'
import { copy } from './copy'
import { canonicalReference, DEFAULT_REGISTRY, parseReference } from './registry'
import { fmtElapsed, type Stage } from './steps'
import { useNow } from '@/lib/useNow'
import { ELAPSED_TICK_MS } from './tuning'
import { openRegistryView } from './follower'
import { dismissImport, importFor, startImport, subscribeUploads } from './uploads'
import { ProgressSegments } from './components/ProgressSegments'
import type { DeployStarted } from './UploadTab'

export function RegistryTab({
  userId,
  onStarted,
}: {
  userId: string
  onStarted?: (to: DeployStarted) => void
}) {
  const ids = useId()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const state = useSyncExternalStore(subscribeUploads, () => importFor(userId))
  const [reference, setReference] = useState(state?.reference ?? '')
  const [formatError, setFormatError] = useState(false)
  const importing = state?.phase === 'importing'
  const clock = useNow(importing ? ELAPSED_TICK_MS : null)

  // Open: this tab shows an import's outcome itself, so the follower doesn't toast it.
  useEffect(() => openRegistryView(), [])

  // Done: the build page when the import built from source, else the agent page (design review 7). A deploy that didn't
  // start (container_name null, D-7) stays here with a link to the agent.
  useEffect(() => {
    if (state?.phase !== 'done' || !state.containerName) return
    dismissImport(userId)
    if (onStarted)
      onStarted(state.buildId ? { buildId: state.buildId } : { agentId: state.agentId })
    else if (state.buildId)
      void navigate({ to: '/builds/$buildId', params: { buildId: state.buildId } })
    else void navigate({ to: '/agents/$agentId', params: { agentId: state.agentId }, search: {} })
  }, [state, userId, navigate, onStarted])

  const err = state?.phase === 'failed' ? state.error : null
  const errText = err ? (typeof err.body === 'string' && err.body ? err.body : err.message) : null
  if (err?.status === 403) {
    return (
      <StateCard
        tone="warning"
        title={copy.registry.disabled}
        action={
          <Button asChild size="sm" variant="outline" className="pointer-coarse:min-h-11">
            <Link to="/deploy" search={{ method: 'upload' }}>
              {copy.github.useUpload}
            </Link>
          </Button>
        }
      >
        {copy.registry.disabledHint}
      </StateCard>
    )
  }
  const fieldError = formatError
    ? copy.registry.format
    : err && [400, 409, 422].includes(err.status)
      ? errText
      : null

  const submit = () => {
    if (!parseReference(reference)) {
      setFormatError(true)
      document.getElementById(`${ids}-ref`)?.focus()
      return
    }
    setFormatError(false)
    dismissImport(userId)
    void startImport(qc, userId, canonicalReference(reference), sendRegistryImport)
  }

  const stages: Stage[] = [
    { id: 'building', label: copy.registry.importing, state: 'current' },
    { id: 'running', label: copy.steps.running, state: 'pending' },
  ]

  return (
    <div className="@container/registry">
      <div className="grid grid-cols-1 gap-4 @[768px]/registry:grid-cols-[minmax(0,3fr)_minmax(0,2fr)] @[768px]/registry:items-start">
        <Card className="gap-4 p-4">
          <form
            noValidate
            className="flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault()
              submit()
            }}
          >
            <Field className="gap-1">
              <FieldLabel htmlFor={`${ids}-ref`}>{copy.registry.reference}</FieldLabel>
              <Input
                id={`${ids}-ref`}
                value={reference}
                readOnly={importing}
                placeholder={copy.registry.placeholder}
                autoComplete="off"
                spellCheck={false}
                className="font-mono"
                onChange={(e) => {
                  setReference(e.target.value)
                  setFormatError(false)
                }}
                aria-invalid={!!fieldError}
                aria-describedby={`${ids}-ref-hint`}
              />
              {fieldError ? (
                <p id={`${ids}-ref-hint`} className="text-xs text-destructive">
                  {fieldError}
                </p>
              ) : (
                <FieldDescription id={`${ids}-ref-hint`} className="text-xs">
                  {copy.registry.hint(DEFAULT_REGISTRY)}
                </FieldDescription>
              )}
            </Field>
            {importing ? (
              <div className="flex flex-col gap-2" data-testid="import-progress">
                <ProgressSegments
                  stages={stages}
                  sub={(s) =>
                    s.state === 'current'
                      ? `${fmtElapsed(clock - state.startedAt)} · ${copy.registry.usual}`
                      : copy.build.pending
                  }
                />
                <p className="text-xs text-muted-foreground">{copy.registry.keepUsing}</p>
              </div>
            ) : (
              <Button type="submit" className="w-full pointer-coarse:min-h-11">
                {copy.registry.submit}
              </Button>
            )}
            {err && !fieldError ? (
              <Alert className="gap-y-1 border-destructive/30 bg-destructive/5 [&>svg]:text-destructive">
                <AlertTriangle aria-hidden />
                <AlertTitle className="line-clamp-none">{copy.deploy.failed}</AlertTitle>
                <AlertDescription>{errText}</AlertDescription>
              </Alert>
            ) : null}
            {state?.phase === 'done' && !state.containerName ? (
              <Alert
                className="gap-y-2 border-warning/40 bg-warning/5 [&>svg]:text-warning"
                data-testid="import-not-running"
              >
                <AlertTriangle aria-hidden />
                <AlertTitle className="line-clamp-none">{copy.registry.notRunning}</AlertTitle>
                <AlertDescription>
                  <Button asChild size="sm" variant="outline" className="pointer-coarse:min-h-11">
                    <Link to="/agents/$agentId" params={{ agentId: state.agentId }} search={{}}>
                      {copy.registry.openAgent}
                    </Link>
                  </Button>
                </AlertDescription>
              </Alert>
            ) : null}
          </form>
        </Card>
        <Card className="gap-2 p-4 @max-[768px]/registry:order-first">
          <h2 className="text-sm font-semibold">{copy.registry.needs}</h2>
          <ul className="divide-y">
            {copy.registry.items.map((t) => (
              <li key={t} className="flex gap-3 py-2 text-sm">
                <Dot aria-hidden className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
                {t}
              </li>
            ))}
          </ul>
        </Card>
      </div>
    </div>
  )
}
