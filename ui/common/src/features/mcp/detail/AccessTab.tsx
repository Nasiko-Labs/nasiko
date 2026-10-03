/**
 * Access (plans/feat-mcp.md §5, legacy "Access & security"; owner or superuser: `/grants` is owner-gated). Public
 * toggle, who has access (`access_reasons`: one row per person with the most specific reason), revoke on direct
 * grants, and grant to a user found through the gateway's own share-target search. An edition layer adds grantee
 * kinds through `connectorGranteeKinds` (EE: teams and departments), each in its own section.
 */
import { RotateCw, Trash2 } from 'lucide-react'
import { useId, useState } from 'react'
import { toast } from 'sonner'
import { useSlots } from '@/app/edition-context'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Field, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { SearchInput } from '@/components/shared/search-input'
import { Section } from '@/features/agents/components/bits'
import { ConfirmDialog } from '@/features/agents/components/dialogs'
import { useGrantWrites, useShares, useShareTargets } from '../api'
import { copy, reason } from '../copy'
import { grantKind } from '../logic'
import { SHARE_SEARCH_MIN } from '../tuning'
import type { AccessReason, ConnectorDetail } from '../types'

export function AccessTab({ connector: c }: { connector: ConnectorDetail }) {
  const id = c.connector_id
  const shares = useShares(id, true)
  const w = useGrantWrites(id)
  const { connectorGranteeKinds } = useSlots()
  const publicId = useId()
  const [filter, setFilter] = useState('')
  const [revoking, setRevoking] = useState<AccessReason | null>(null)

  const term = filter.trim().toLowerCase()
  const rows = (shares.data?.access_reasons ?? []).filter(
    (r) =>
      !term ||
      `${r.display_name ?? ''} ${r.username} ${r.email ?? ''}`.toLowerCase().includes(term),
  )
  const nameOf = (r: AccessReason) => r.display_name || r.username

  return (
    <div className="space-y-4">
      <Section title={copy.publicTitle}>
        {shares.isPending ? (
          <Skeleton className="h-8" />
        ) : shares.isError ? null : (
          <Field orientation="horizontal" className="gap-2">
            <Checkbox
              id={publicId}
              checked={shares.data.is_public}
              disabled={w.setPublic.isPending}
              onCheckedChange={(v) =>
                w.setPublic.mutate(v === true, {
                  onError: (e) => toast.error(copy.grantFailed(reason(e))),
                })
              }
            />
            <FieldLabel htmlFor={publicId} className="font-normal">
              {copy.publicToggle}
            </FieldLabel>
          </Field>
        )}
      </Section>

      <Section title={copy.grantsTitle}>
        <p className="text-sm text-muted-foreground">{copy.grantsSub}</p>
        {shares.isPending ? (
          <Skeleton className="h-24" />
        ) : shares.isError ? (
          <p role="alert" className="text-sm text-destructive">
            {copy.grantsFailed}: {reason(shares.error)}{' '}
            <Button
              size="sm"
              variant="outline"
              className="ml-2 h-7"
              onClick={() => void shares.refetch()}
            >
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          </p>
        ) : (
          <>
            <SearchInput
              aria-label={copy.searchGrants}
              placeholder={copy.searchGrants}
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              className="w-full sm:w-72"
            />
            {rows.length ? (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>{copy.grantCols.user}</TableHead>
                    <TableHead>{copy.grantCols.email}</TableHead>
                    <TableHead>{copy.grantCols.role}</TableHead>
                    <TableHead>{copy.grantCols.grant}</TableHead>
                    <TableHead className="w-10">
                      <span className="sr-only">{copy.revokeTitle}</span>
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((r) => {
                    const kind = grantKind(r.via)
                    return (
                      <TableRow key={r.user_id}>
                        <TableCell className="font-medium">{nameOf(r)}</TableCell>
                        <TableCell className="text-muted-foreground">{r.email || '—'}</TableCell>
                        <TableCell className="text-muted-foreground">{r.role || '—'}</TableCell>
                        <TableCell>
                          <Badge
                            variant={
                              kind === 'owner' ? 'default' : kind === 'direct' ? 'info' : 'outline'
                            }
                            title={r.via_label ?? undefined}
                          >
                            {copy.grantKind[kind]}
                          </Badge>
                        </TableCell>
                        <TableCell>
                          {kind === 'direct' ? (
                            <Button
                              size="icon-sm"
                              variant="ghost"
                              aria-label={copy.revokeNamed(nameOf(r))}
                              title={copy.revokeNamed(nameOf(r))}
                              onClick={() => setRevoking(r)}
                            >
                              <Trash2 className="size-4 text-destructive" aria-hidden />
                            </Button>
                          ) : null}
                        </TableCell>
                      </TableRow>
                    )
                  })}
                </TableBody>
              </Table>
            ) : (
              <p className="text-sm text-muted-foreground">
                {term ? copy.noGrantMatch : copy.noGrants}
              </p>
            )}
            <UserPicker
              exclude={new Set((shares.data.access_reasons ?? []).map((r) => r.user_id))}
              pending={w.add.isPending}
              onPick={(u) =>
                w.add.mutate(
                  { kind: 'users', granteeId: u.id },
                  {
                    onSuccess: () => toast.success(copy.granted(u.label)),
                    onError: (e) => toast.error(copy.grantFailed(reason(e))),
                  },
                )
              }
            />
          </>
        )}
      </Section>

      {connectorGranteeKinds.map((k) => (
        <Section key={k.key} title={k.label}>
          <k.Section connectorId={id} />
        </Section>
      ))}

      <ConfirmDialog
        open={!!revoking}
        onOpenChange={(o) => (o ? null : setRevoking(null))}
        title={copy.revokeTitle}
        body={revoking ? copy.revokeBody(nameOf(revoking)) : ''}
        confirmLabel={copy.revoke}
        destructive
        pending={w.revoke.isPending}
        error={w.revoke.error}
        onConfirm={() =>
          revoking &&
          w.revoke.mutate(
            { kind: 'users', granteeId: revoking.user_id },
            {
              onSuccess: () => {
                setRevoking(null)
                toast.success(copy.revoked)
              },
            },
          )
        }
      />
    </div>
  )
}

function UserPicker({
  exclude,
  pending,
  onPick,
}: {
  exclude: ReadonlySet<string>
  pending: boolean
  onPick: (u: { id: string; label: string }) => void
}) {
  const inputId = useId()
  const [q, setQ] = useState('')
  const search = useShareTargets(q)
  const hits = (search.data ?? []).filter((u) => !exclude.has(u.user_id))
  const short = q.trim().length < SHARE_SEARCH_MIN
  return (
    <div className="space-y-2 pt-2">
      <Field className="max-w-sm gap-2">
        <FieldLabel htmlFor={inputId}>{copy.grantTitle}</FieldLabel>
        <Input
          id={inputId}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder={copy.searchUsers}
          autoComplete="off"
        />
      </Field>
      {q && short ? (
        <p className="text-xs text-muted-foreground">{copy.typeMore(SHARE_SEARCH_MIN)}</p>
      ) : null}
      {!short && search.isError ? (
        <p role="alert" className="text-xs text-destructive">
          {reason(search.error)}
        </p>
      ) : null}
      {!short && search.isSuccess && !hits.length ? (
        <p className="text-xs text-muted-foreground">{copy.noUsers}</p>
      ) : null}
      {hits.length ? (
        <ul className="max-w-sm divide-y divide-border rounded-md border border-border text-sm">
          {hits.map((u) => {
            const label = u.display_name || u.username
            return (
              <li key={u.user_id} className="flex items-center justify-between px-2 py-1">
                <span>
                  {label}{' '}
                  {u.display_name ? (
                    <span className="text-xs text-muted-foreground">{u.username}</span>
                  ) : null}
                </span>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={pending}
                  aria-label={copy.grantNamed(label)}
                  onClick={() => {
                    onPick({ id: u.user_id, label })
                    setQ('')
                  }}
                >
                  {copy.grant}
                </Button>
              </li>
            )
          })}
        </ul>
      ) : null}
    </div>
  )
}
