/**
 * Your configs (plan §4.1): each config's routing sentence, key, "Used by N" and a row menu (Edit, default,
 * Duplicate, Delete, CLI equivalent). Configs made elsewhere show as stored, with their warnings.
 */
import { AlertTriangle, MoreHorizontal, Waypoints } from 'lucide-react'
import { EmptyState } from '@nasiko/ui/composites/state-card'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@nasiko/ui/primitives/dropdown-menu'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { CopyMenuItem } from '@nasiko/ui/composites/copy-button'
import { copy } from '../copy'
import { cliLine, countText, keySource, routingSentence, usedBy, type RowRead } from '../routing'
import type { LlmConfig } from '../types'
import { KeySourceChip, SentenceText } from './bits'
import { SectionError } from './SectionError'

export function ConfigsSection({
  configs,
  reads,
  warnings,
  onNew,
  onEdit,
  onDuplicate,
  onDelete,
  onDefault,
}: {
  configs: {
    data?: LlmConfig[]
    isPending: boolean
    isError: boolean
    error: unknown
    refetch: () => unknown
  }
  reads: readonly RowRead[]
  warnings: (c: LlmConfig) => string[]
  onNew: () => void
  onEdit: (c: LlmConfig) => void
  onDuplicate: (c: LlmConfig) => void
  onDelete: (c: LlmConfig) => void
  onDefault: (c: LlmConfig, on: boolean) => void
}) {
  const list = configs.data ?? []
  const hasDefault = list.some((c) => c.is_default)
  return (
    <section id="router-configs" aria-labelledby="router-configs-h" className="scroll-mt-4">
      <h2 id="router-configs-h" tabIndex={-1} className="sr-only">
        {copy.configsTitle}
      </h2>
      {/* The toolbar is the list's top edge: what is here on the left, what to do with it on the right. */}
      <div className="rounded-lg border border-border bg-card">
        <div className="flex flex-wrap items-center gap-2 border-b border-border p-2 pl-4">
          <span className="text-sm text-muted-foreground">
            {configs.data ? copy.configsCount(list.length) : null}
          </span>
          <div className="ml-auto flex gap-2">
            {list.length && !hasDefault ? (
              <SetADefault configs={list} onDefault={onDefault} />
            ) : null}
            <Button size="sm" onClick={onNew} className="pointer-coarse:min-h-11">
              {copy.newConfig}
            </Button>
          </div>
        </div>
        {configs.isPending ? (
          <div className="space-y-2 p-2" aria-busy="true">
            {[0, 1, 2].map((i) => (
              <Skeleton key={i} className="h-11" />
            ))}
          </div>
        ) : configs.isError && !configs.data ? (
          <div className="p-2">
            <SectionError error={configs.error} onRetry={() => void configs.refetch()} />
          </div>
        ) : list.length === 0 ? (
          <EmptyState
            icon={Waypoints}
            title={copy.noConfigs}
            className="border-0"
            action={
              <Button className="pointer-coarse:min-h-11" size="sm" onClick={onNew}>
                {copy.createFirst}
              </Button>
            }
          >
            {copy.noConfigsText}
          </EmptyState>
        ) : (
          <ul className="divide-y divide-border" aria-label={copy.configsTitle}>
            {list.map((c) => (
              <li
                key={c.id}
                id={`config-${c.id}`}
                tabIndex={-1}
                className="grid scroll-mt-4 gap-1.5 px-4 py-2.5 text-sm sm:grid-cols-[minmax(0,1.3fr)_minmax(0,2fr)_minmax(0,1.2fr)_6rem_2.5rem] sm:items-center sm:gap-3"
              >
                <div className="flex min-w-0 flex-wrap items-center gap-2">
                  <span className="truncate font-medium">{c.name}</span>
                  {c.is_default ? <Badge variant="secondary">{copy.defaultBadge}</Badge> : null}
                </div>
                <div className="min-w-0 text-muted-foreground">
                  <SentenceText sentence={routingSentence(c, null)} />
                  {warnings(c).map((w) => (
                    <span key={w} className="mt-0.5 flex items-center gap-1 text-xs text-warning">
                      <AlertTriangle className="size-3.5 shrink-0" aria-hidden /> {w}
                    </span>
                  ))}
                </div>
                <div className="min-w-0">
                  <KeySourceChip source={keySource(c)} />
                </div>
                <div className="text-xs text-muted-foreground">
                  {copy.usedBy(countText(usedBy(c, reads)))}
                </div>
                <div className="sm:text-right">
                  <RowMenu
                    config={c}
                    onEdit={onEdit}
                    onDuplicate={onDuplicate}
                    onDelete={onDelete}
                    onDefault={onDefault}
                  />
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  )
}

function RowMenu({
  config,
  onEdit,
  onDuplicate,
  onDelete,
  onDefault,
}: {
  config: LlmConfig
  onEdit: (c: LlmConfig) => void
  onDuplicate: (c: LlmConfig) => void
  onDelete: (c: LlmConfig) => void
  onDefault: (c: LlmConfig, on: boolean) => void
}) {
  const cli = cliLine(config)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            size="sm"
            variant="ghost"
            className="size-8 px-0 pointer-coarse:size-11"
            aria-label={copy.configActions(config.name)}
          />
        }
      >
        <MoreHorizontal className="size-4" aria-hidden />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-72">
        <DropdownMenuItem onClick={() => onEdit(config)}>{copy.edit}</DropdownMenuItem>
        <DropdownMenuItem onClick={() => onDefault(config, !config.is_default)}>
          {config.is_default ? copy.removeDefault : copy.setDefault}
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => onDuplicate(config)}>{copy.duplicate}</DropdownMenuItem>
        <DropdownMenuSeparator />
        {'line' in cli ? (
          <CopyMenuItem text={cli.line} label={copy.copyCli} />
        ) : (
          <p className="px-2 py-1.5 text-xs text-muted-foreground">
            {cli.unsupported === 'tiered' ? copy.cliTiered : copy.cliNoModel}
          </p>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem className="text-destructive" onClick={() => onDelete(config)}>
          {copy.delete}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function SetADefault({
  configs,
  onDefault,
}: {
  configs: LlmConfig[]
  onDefault: (c: LlmConfig, on: boolean) => void
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={<Button size="sm" variant="outline" className="pointer-coarse:min-h-11" />}
      >
        {copy.setADefault}
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {configs.map((c) => (
          <DropdownMenuItem key={c.id} onClick={() => onDefault(c, true)}>
            {c.name}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
