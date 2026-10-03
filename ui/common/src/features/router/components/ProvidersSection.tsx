/**
 * Providers (plan §4.1, §4.6): the catalog with prices, custom providers (sync status; superuser actions inline)
 * and the read-only tier registry. A provider the router can't call (not built in, not a registered custom
 * provider) says so instead of looking usable.
 */
import { AlertTriangle, MoreHorizontal } from 'lucide-react'
import { useState } from 'react'
import { EmptyState } from '@/components/shared/state-card'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { ConfirmDialog } from '@/features/agents/components/dialogs'
import { relTime } from '@/features/agents/format'
import { cn } from '@/lib/utils'
import { useAnnounce } from '../announce'
import { referencingConfigs, useDeleteCustomProvider, useSyncCustomProvider } from '../api'
import { copy } from '../copy'
import { routerError } from '../errors'
import { isRoutable } from '../routing'
import type { CustomProvider, ModelMapping, ProviderCatalog } from '../types'
import { SectionError } from './SectionError'

interface Q<T> {
  data?: T
  isPending: boolean
  isError: boolean
  error: unknown
  refetch: () => unknown
}

// A table from sm; below it each provider stacks and the header is hidden.
const ROW = 'text-sm hover:bg-transparent max-sm:grid max-sm:gap-1.5 max-sm:px-4 max-sm:py-2.5'
const CELL =
  'p-0 align-top whitespace-normal max-sm:block sm:px-1.5 sm:py-2.5 sm:first:pl-4 sm:last:pr-4'
const HEAD = 'h-auto px-1.5 py-2 text-xs text-muted-foreground first:pl-4 last:pr-4'

const price = (v: number | null | undefined) => (v === null || v === undefined ? null : `$${v}`)

export function ProvidersSection({
  catalog,
  custom,
  registry,
  superuser,
  onAdd,
  onEdit,
}: {
  catalog: Q<ProviderCatalog[]>
  custom: Q<CustomProvider[]>
  registry: Q<ModelMapping[]>
  superuser: boolean
  onAdd: () => void
  onEdit: (p: CustomProvider) => void
}) {
  const customs = custom.data ?? []
  const labels = customs.map((c) => c.label)
  const byLabel = new Map(customs.map((c) => [c.label, c]))
  // Custom providers with nothing in the catalog yet (never synced) still get a row.
  const groups = [...(catalog.data ?? [])]
  for (const c of customs)
    if (!groups.some((g) => g.provider === c.label))
      groups.push({
        provider: c.label,
        provider_id: c.id,
        display_name: c.display_name,
        models: [],
      })
  return (
    <section
      id="router-providers"
      aria-labelledby="router-providers-h"
      className="scroll-mt-4 space-y-2"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="router-providers-h" tabIndex={-1} className="text-sm font-semibold">
          {copy.providersTitle}
        </h2>
        {superuser ? (
          <Button size="sm" variant="outline" onClick={onAdd} className="pointer-coarse:min-h-11">
            {copy.addCustom}
          </Button>
        ) : null}
      </div>
      {catalog.isPending ? (
        <Skeleton className="h-32" />
      ) : catalog.isError && !catalog.data ? (
        <SectionError
          title={copy.catalogFailed}
          error={catalog.error}
          onRetry={() => void catalog.refetch()}
        />
      ) : (
        <div className="rounded-lg border border-border bg-card">
          <Table aria-label={copy.providersTitle} className="max-sm:block sm:table-fixed">
            <TableHeader className="max-sm:hidden">
              <TableRow className="hover:bg-transparent">
                <TableHead className={cn(HEAD, 'w-1/3')}>{copy.colProvider}</TableHead>
                <TableHead className={HEAD}>
                  {copy.colModels} · {copy.colPrice}
                </TableHead>
                <TableHead className={cn(HEAD, 'w-14')}>
                  <span className="sr-only">{copy.colActions}</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody className="max-sm:block">
              {groups.map((g) => (
                <ProviderRow
                  key={g.provider}
                  group={g}
                  custom={byLabel.get(g.provider)}
                  routable={isRoutable(g.provider, labels)}
                  superuser={superuser}
                  onEdit={onEdit}
                />
              ))}
            </TableBody>
          </Table>
          {superuser && !customs.length && custom.data ? (
            <p className="border-t border-border px-4 py-2 text-xs text-muted-foreground">
              {copy.noCustom}
            </p>
          ) : null}
          {custom.isError ? (
            <div className="border-t border-border p-2">
              <SectionError
                error={custom.error}
                onRetry={() => void custom.refetch()}
                title={copy.customProviders}
              />
            </div>
          ) : null}
        </div>
      )}
      <Registry registry={registry} />
    </section>
  )
}

function ProviderRow({
  group,
  custom,
  routable,
  superuser,
  onEdit,
}: {
  group: ProviderCatalog
  custom?: CustomProvider
  routable: boolean
  superuser: boolean
  onEdit: (p: CustomProvider) => void
}) {
  return (
    <TableRow className={ROW}>
      <TableCell className={cn(CELL, 'min-w-0 space-y-1')}>
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-medium">{group.display_name || group.provider}</span>
          {custom ? <Badge variant="outline">{copy.customBadge}</Badge> : null}
        </div>
        {custom ? <p className="font-mono text-xs text-muted-foreground">{custom.label}</p> : null}
        {custom ? <SyncStatus p={custom} /> : null}
        {!routable ? (
          <p className="flex items-start gap-1 text-xs text-warning">
            <AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden />{' '}
            {copy.notRoutable(group.provider)}
          </p>
        ) : null}
      </TableCell>
      <TableCell className={cn(CELL, 'min-w-0')}>
        {group.models.length ? (
          <ul className="flex flex-wrap gap-x-4 gap-y-1 text-xs">
            {group.models.map((m) => (
              <li key={m.model} className="font-mono">
                {m.model}{' '}
                <span className="font-sans text-muted-foreground">
                  {m.pricing_available
                    ? `${price(m.input_price_per_1m)} / ${price(m.output_price_per_1m)}`
                    : copy.noPrice}
                </span>
              </li>
            ))}
          </ul>
        ) : (
          <span className="text-xs text-muted-foreground">{copy.noModels}</span>
        )}
      </TableCell>
      <TableCell className={cn(CELL, 'sm:text-right')}>
        {custom && superuser ? <CustomMenu p={custom} onEdit={onEdit} /> : null}
      </TableCell>
    </TableRow>
  )
}

function SyncStatus({ p }: { p: CustomProvider }) {
  const text =
    p.last_sync_status === 'error' || p.last_sync_error
      ? copy.sync.failed(p.last_sync_error ?? 'unknown error')
      : p.last_sync_at
        ? copy.sync.synced(relTime(p.last_sync_at))
        : copy.sync.never
  return (
    <p className={`text-xs ${p.last_sync_error ? 'text-warning' : 'text-muted-foreground'}`}>
      {text}
    </p>
  )
}

function CustomMenu({ p, onEdit }: { p: CustomProvider; onEdit: (p: CustomProvider) => void }) {
  const sync = useSyncCustomProvider()
  const del = useDeleteCustomProvider()
  const announce = useAnnounce()
  const [confirming, setConfirming] = useState(false)
  const refs = referencingConfigs(del.error)
  return (
    <div className="space-y-1">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            size="sm"
            variant="ghost"
            className="size-8 px-0 pointer-coarse:size-11"
            aria-label={copy.configActions(p.display_name)}
          >
            <MoreHorizontal className="size-4" aria-hidden />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onSelect={() => onEdit(p)}>{copy.edit}</DropdownMenuItem>
          <DropdownMenuItem
            disabled={sync.isPending}
            onSelect={() =>
              sync.mutate(p.id, {
                onSuccess: (r) => announce(copy.syncDone(p.display_name, r.discovered_models)),
                onError: (e) => announce(routerError(e).problem),
              })
            }
          >
            {sync.isPending ? copy.sync.syncing : copy.syncNow}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className="text-destructive"
            onSelect={() => {
              del.reset()
              setConfirming(true)
            }}
          >
            {copy.delete}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {sync.isError ? (
        <p className="text-xs text-destructive">{routerError(sync.error).problem}</p>
      ) : null}
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title={copy.deleteTitle(p.display_name)}
        body={copy.deleteBody}
        confirmLabel={copy.delete}
        destructive
        pending={del.isPending}
        onConfirm={() =>
          del.mutate(p.id, {
            onSuccess: () => {
              setConfirming(false)
              announce(copy.deleted(p.display_name))
            },
          })
        }
      >
        {refs ? (
          <p role="alert" className="text-sm text-destructive">
            {copy.cpDeleteRefs(refs.join(', '))}
          </p>
        ) : null}
        {del.isError && !refs ? (
          <p role="alert" className="text-sm text-destructive">
            {routerError(del.error).problem}
          </p>
        ) : null}
      </ConfirmDialog>
    </div>
  )
}

const TIER_HEAD = 'h-auto px-0 py-1 pr-2 text-xs text-muted-foreground'

function Registry({ registry }: { registry: Q<ModelMapping[]> }) {
  if (registry.isPending) return <Skeleton className="h-16" />
  if (registry.isError && !registry.data)
    return (
      <SectionError
        title={copy.registryTitle}
        error={registry.error}
        onRetry={() => void registry.refetch()}
      />
    )
  const rows = registry.data ?? []
  const providers = [...new Set(rows.map((r) => r.provider))]
  return (
    <div className="rounded-lg border border-border bg-card p-4 text-sm">
      <h3 className="text-sm font-medium">{copy.registryTitle}</h3>
      <p className="text-xs text-muted-foreground">{copy.registryWayOut}</p>
      {rows.length === 0 ? (
        <EmptyState className="mt-2" title={copy.registryEmpty} />
      ) : (
        <Table className="mt-2 text-xs">
          <TableHeader>
            <TableRow className="hover:bg-transparent">
              <TableHead className={TIER_HEAD}>{copy.colProvider}</TableHead>
              {copy.tierLabels.map((t) => (
                <TableHead key={t} className={TIER_HEAD}>
                  {t}
                </TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {providers.map((p) => (
              <TableRow key={p} className="border-t border-border hover:bg-transparent">
                <TableCell className="px-0 py-1 pr-2">{p}</TableCell>
                {[1, 2, 3].map((tier) => (
                  <TableCell key={tier} className="px-0 py-1 pr-2 font-mono">
                    {rows.find((r) => r.provider === p && r.tier === tier)?.model ?? '—'}
                  </TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </div>
  )
}
