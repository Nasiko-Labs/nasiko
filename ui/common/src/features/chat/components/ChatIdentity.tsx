/**
 * The chat identity's marks (v1c §5.2): one icon per kind, the same in the rail row, the header and
 * the composer target. The agent status dot shows in the header, picker and target list, never on the
 * 20 px rail icon (DS2).
 */
import { BotOff, Route, SquareTerminal } from 'lucide-react'
import { AgentLinkTo } from '@/features/agents/components/bits'
import { cn } from '@/lib/utils'
import { copy } from '../copy'
import type { ChatIdentity, IdentityKind } from '../identity'

const BOX: Record<16 | 20 | 32, string> = {
  16: 'size-4 rounded text-5xs',
  20: 'size-5 rounded-sm text-4xs',
  32: 'size-8 rounded-md text-xs',
}
const GLYPH: Record<16 | 20 | 32, string> = { 16: 'size-3', 20: 'size-3.5', 32: 'size-5' }

const monogram = (name: string) =>
  name
    .split(/[\s_.-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0].toUpperCase())
    .join('') || '?'

export function IdentityIcon({
  kind,
  name,
  size = 20,
  className,
}: {
  kind: IdentityKind
  name: string
  size?: 16 | 20 | 32
  className?: string
}) {
  const base = cn(
    'inline-flex shrink-0 items-center justify-center font-semibold',
    BOX[size],
    className,
  )
  switch (kind) {
    case 'agent':
      return (
        <span aria-hidden className={cn(base, 'bg-muted text-foreground')}>
          {monogram(name)}
        </span>
      )
    case 'orchestrator':
      return (
        <span aria-hidden className={cn(base, 'bg-primary/10 text-primary-text')}>
          <Route className={GLYPH[size]} />
        </span>
      )
    case 'recorded':
      return (
        <span aria-hidden className={cn(base, 'bg-muted text-foreground')}>
          <SquareTerminal className={GLYPH[size]} />
        </span>
      )
    case 'removed':
      return (
        <span aria-hidden className={cn(base, 'bg-muted text-muted-foreground')}>
          <BotOff className={GLYPH[size]} />
        </span>
      )
  }
}

/** Running or not, by shape as well as colour (filled vs ring) and in words for screen readers. */
export function StatusDot({ running }: { running: boolean }) {
  return (
    <>
      <span
        aria-hidden
        className={
          running
            ? 'size-2 shrink-0 rounded-full bg-success'
            : 'size-2 shrink-0 rounded-full border border-muted-foreground'
        }
      />
      <span className="sr-only">{running ? copy.statusRunning : copy.statusNotRunning}</span>
    </>
  )
}

/**
 * The header chip (§5.5): agent = status dot + name linking to the agent; Orchestrator = its icon and
 * name; recorded = "Recorded". The §5.2 subline is the chip's description, read after its name (DS5).
 */
export function IdentityChip({
  identity,
  className,
}: {
  identity: ChatIdentity
  className?: string
}) {
  const chip = cn(
    'inline-flex h-6 max-w-[24ch] shrink-0 items-center gap-1.5 rounded-full border border-border px-2 text-xs',
    className,
  )
  const description = <span className="sr-only">, {identity.subline}</span>
  switch (identity.kind) {
    case 'agent':
      return (
        <span className={chip} data-testid="identity-chip">
          {identity.status ? <StatusDot running={identity.status === 'running'} /> : null}
          {identity.agentId ? (
            <AgentLinkTo
              id={identity.agentId}
              className="inline-flex items-center truncate pointer-coarse:min-h-11"
            >
              {identity.name}
            </AgentLinkTo>
          ) : (
            <span className="truncate">{identity.name}</span>
          )}
          {description}
        </span>
      )
    case 'orchestrator':
      return (
        <span className={chip} data-testid="identity-chip" title={copy.routedBadgeTooltip}>
          <Route aria-hidden className="size-3 text-primary-text" />
          <span className="truncate">{identity.name}</span>
          {description}
        </span>
      )
    case 'recorded':
      return (
        <span className={chip} data-testid="identity-chip">
          <span className="truncate">{copy.recordedBadge}</span>
          {description}
        </span>
      )
    case 'removed':
      return (
        <span className={cn(chip, 'text-muted-foreground')} data-testid="identity-chip">
          <span className="truncate">{identity.name}</span>
          {description}
        </span>
      )
  }
}
