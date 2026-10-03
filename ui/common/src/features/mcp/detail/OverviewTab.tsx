/**
 * Overview (plans/feat-mcp.md §5, legacy detail page Overview): facts, the caller's own connection to this server
 * (credential or OAuth), its tools, and the owner's danger zone.
 */
import { ExternalLink, KeyRound, ShieldCheck, Trash2 } from 'lucide-react'
import { useId, useState } from 'react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Field, FieldError, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { SearchInput } from '@/components/shared/search-input'
import { Section } from '@/features/agents/components/bits'
import { useCredentialStatus, useCredentialWrites, useOauthStatus, useOauthWrites } from '../api'
import { AUTH_LABEL, copy, reason } from '../copy'
import { DeleteDialog } from '../components/DeleteDialog'
import { labelOf } from '../logic'
import { TOOL_FILTER_FROM } from '../tuning'
import type { ConnectorDetail } from '../types'

const fmtDate = (v: string | null | undefined) => (v ? new Date(v).toLocaleString() : '—')

export function OverviewTab({
  connector: c,
  manage,
  openPopup,
}: {
  connector: ConnectorDetail
  manage: boolean
  openPopup: (url: string | undefined) => void
}) {
  const [delOpen, setDelOpen] = useState(false)
  const uploaded = c.source_kind === 'uploaded_build'
  const facts: [string, string][] = [
    [copy.factLabels.url, c.url || '—'],
    [copy.factLabels.transport, c.transport || '—'],
    [copy.factLabels.auth, AUTH_LABEL[c.auth_type ?? ''] ?? c.auth_type ?? '—'],
    [copy.factLabels.version, c.version || c.upload_info?.version || '—'],
    [copy.factLabels.source, uploaded ? copy.uploaded : copy.registeredSource],
    [copy.factLabels.owner, c.owner_username || '—'],
    [copy.factLabels.tools, String(c.tool_count)],
    [copy.factLabels.created, fmtDate(c.created_at)],
  ]
  return (
    <div className="space-y-4">
      {c.description ? <p className="text-sm text-muted-foreground">{c.description}</p> : null}
      <Section title={copy.facts}>
        <dl className="grid gap-x-6 gap-y-3 sm:grid-cols-2 lg:grid-cols-4">
          {facts.map(([k, v]) => (
            <div key={k} className="min-w-0">
              <dt className="text-xs text-muted-foreground">{k}</dt>
              <dd className="truncate text-sm" title={v}>
                {v}
              </dd>
            </div>
          ))}
        </dl>
        {c.upload_info?.error_msg ? (
          <p role="alert" className="text-sm text-destructive">
            {copy.buildError(c.upload_info.error_msg)}
          </p>
        ) : null}
      </Section>
      <Section title={copy.connection}>
        <Connection connector={c} openPopup={openPopup} />
      </Section>
      <Tools connector={c} />
      {manage ? (
        <Section title={copy.danger} className="border-destructive/40">
          <p className="text-sm text-muted-foreground">{copy.dangerBody}</p>
          <div>
            <Button
              variant="outline"
              size="sm"
              className="text-destructive"
              onClick={() => setDelOpen(true)}
            >
              <Trash2 aria-hidden /> {copy.delete}
            </Button>
          </div>
          <DeleteDialog
            id={c.connector_id}
            label={labelOf(c)}
            open={delOpen}
            onOpenChange={setDelOpen}
          />
        </Section>
      ) : null}
    </div>
  )
}

/** The caller's own credential or OAuth token for this server (per user, write-only on the server). */
function Connection({
  connector: c,
  openPopup,
}: {
  connector: ConnectorDetail
  openPopup: (url: string | undefined) => void
}) {
  if (c.auth_type === 'oauth2') return <OauthPanel id={c.connector_id} openPopup={openPopup} />
  if (c.auth_type && c.auth_type !== 'none')
    return <CredentialPanel id={c.connector_id} basic={c.auth_type === 'basic'} />
  return <p className="text-sm text-muted-foreground">{copy.noAuthNeeded}</p>
}

function CredentialPanel({ id, basic }: { id: string; basic: boolean }) {
  const inputId = useId()
  const status = useCredentialStatus(id, true)
  const w = useCredentialWrites(id)
  const [value, setValue] = useState('')
  const [note, setNote] = useState<string | null>(null)
  if (status.isPending) return <Skeleton className="h-16" />
  const set = !!status.data?.connected
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <KeyRound className="size-4 text-muted-foreground" aria-hidden />
        {status.isError ? (
          <span className="text-sm text-destructive">{copy.statusFailed}</span>
        ) : (
          <Badge variant={set ? 'success' : 'muted'}>
            {set ? copy.credentialSet : copy.noCredential}
          </Badge>
        )}
        {set ? (
          <Button
            size="sm"
            variant="outline"
            disabled={w.remove.isPending}
            onClick={() =>
              w.remove.mutate(undefined, {
                onSuccess: () => setNote(null),
                onError: (e) => toast.error(copy.removeFailed(reason(e))),
              })
            }
          >
            {copy.remove}
          </Button>
        ) : null}
      </div>
      <form
        className="flex max-w-lg flex-wrap items-end gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          if (!value.trim()) return
          setNote(null)
          w.save.mutate(value.trim(), {
            onSuccess: (r) => {
              setValue('')
              if (r.connected) toast.success(copy.credentialSaved)
              else setNote(copy.verifyFailed(r.error))
            },
          })
        }}
      >
        <Field data-invalid={w.save.isError || !!note} className="min-w-0 flex-1 gap-1.5">
          <FieldLabel htmlFor={inputId}>{copy.credentialLabel}</FieldLabel>
          <Input
            id={inputId}
            type="password"
            autoComplete="off"
            placeholder={basic ? copy.basicPlaceholder : copy.keyLabel}
            value={value}
            onChange={(e) => setValue(e.target.value)}
          />
        </Field>
        <Button type="submit" disabled={w.save.isPending || !value.trim()}>
          {set ? copy.replace : copy.save}
        </Button>
      </form>
      {note ? <FieldError>{note}</FieldError> : null}
      {w.save.isError ? <FieldError>{copy.saveFailed(reason(w.save.error))}</FieldError> : null}
    </div>
  )
}

function OauthPanel({
  id,
  openPopup,
}: {
  id: string
  openPopup: (url: string | undefined) => void
}) {
  const status = useOauthStatus(id, true)
  const w = useOauthWrites(id)
  if (status.isPending) return <Skeleton className="h-10" />
  const s = status.data
  return (
    <div className="flex flex-wrap items-center gap-2">
      <ShieldCheck className="size-4 text-muted-foreground" aria-hidden />
      <span className="text-sm font-medium">{copy.oauthTitle}</span>
      {status.isError ? (
        <span className="text-sm text-destructive">{copy.statusFailed}</span>
      ) : (
        <Badge variant={s?.authorized ? 'success' : 'muted'}>
          {s?.authorized
            ? `${copy.authorized}${s.expires_at ? ` · ${copy.expires(fmtDate(s.expires_at))}` : ''}`
            : copy.notAuthorized}
        </Badge>
      )}
      {s?.authorized ? (
        <Button
          size="sm"
          variant="outline"
          disabled={w.revoke.isPending}
          onClick={() =>
            w.revoke.mutate(undefined, {
              onError: (e) => toast.error(copy.revokeFailed(reason(e))),
            })
          }
        >
          {copy.revoke}
        </Button>
      ) : (
        <Button
          size="sm"
          disabled={w.authorize.isPending}
          onClick={() =>
            w.authorize.mutate(undefined, {
              onSuccess: (r) => openPopup(r.authorization_url),
              onError: (e) => toast.error(copy.authorizeFailed(reason(e))),
            })
          }
        >
          <ExternalLink aria-hidden /> {copy.authorize}
        </Button>
      )}
    </div>
  )
}

function Tools({ connector: c }: { connector: ConnectorDetail }) {
  const [q, setQ] = useState('')
  const term = q.trim().toLowerCase()
  const shown = term
    ? c.tools.filter((t) => `${t.name} ${t.description ?? ''}`.toLowerCase().includes(term))
    : c.tools
  return (
    <Section title={copy.toolsTitle(c.tools.length)}>
      {!c.tools.length ? (
        <p className="text-sm text-muted-foreground">{copy.noTools}</p>
      ) : (
        <>
          {c.tools.length >= TOOL_FILTER_FROM ? (
            <SearchInput
              aria-label={copy.filterTools}
              placeholder={copy.filterTools}
              value={q}
              onChange={(e) => setQ(e.target.value)}
              className="w-full sm:w-72"
            />
          ) : null}
          {shown.length ? (
            <ul className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
              {shown.map((t) => (
                <li key={t.name} className="rounded-md border border-border p-3">
                  <div className="font-mono text-sm">{t.name}</div>
                  {t.description ? (
                    <p className="mt-1 text-xs text-muted-foreground">{t.description}</p>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-sm text-muted-foreground">{copy.noToolMatch}</p>
          )}
        </>
      )}
    </Section>
  )
}
