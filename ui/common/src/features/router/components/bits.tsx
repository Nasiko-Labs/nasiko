/**
 * Small shared pieces of the router page: the KeySource chip, source badges, the routing sentence and the
 * page's polite announcer (plan §4.7). Labels are never colour-only.
 */
import { AlertTriangle, Building2, KeyRound } from 'lucide-react'
import { useCallback, useState, type ComponentProps, type ReactNode } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { AnnounceContext } from '../announce'
import { copy } from '../copy'
import { sourceLabel } from '../format'
import type { KeySource, Sentence } from '../routing'
import type { RoutingSource } from '../types'

export function KeySourceChip({ source, className }: { source: KeySource; className?: string }) {
  const user = source.kind === 'user'
  const Icon = user ? KeyRound : Building2
  return (
    <span
      title={user ? copy.yourKey(source.secret) : copy.platformKey}
      className={cn(
        'inline-flex max-w-full items-center gap-1 rounded-md border border-border bg-muted px-1.5 py-0.5 text-xs text-muted-foreground',
        className,
      )}
    >
      <Icon className="size-3 shrink-0" aria-hidden />
      <span className="truncate">{user ? copy.yourKey(source.secret) : copy.platformKey}</span>
    </span>
  )
}

/** A router warning (DESIGN.md "Router surfaces"): a 14 px triangle in --warning beside the text, never colour alone. */
export function Warn({
  children,
  className,
  id,
  testId,
}: {
  children: ReactNode
  className?: string
  id?: string
  testId?: string
}) {
  return (
    <p
      id={id}
      data-testid={testId}
      className={cn('flex items-start gap-1 text-xs text-warning', className)}
    >
      <AlertTriangle className="mt-px size-3.5 shrink-0" aria-hidden />
      <span>{children}</span>
    </p>
  )
}

export function SourceBadge({
  source,
  viewerIsOwner = true,
}: {
  source: RoutingSource
  viewerIsOwner?: boolean
}) {
  return (
    <Badge variant="outline" data-source={source}>
      {sourceLabel(source, viewerIsOwner)}
    </Badge>
  )
}

/** The row form of a routing sentence; a tiered row lists its models in a tooltip. */
export function SentenceText({
  sentence,
  full = false,
  className,
}: {
  sentence: Sentence
  full?: boolean
  className?: string
}) {
  if (full || sentence.kind !== 'tiered')
    return <span className={className}>{full ? sentence.full : sentence.short}</span>
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <LinkButton
          className={cn('font-normal text-inherit underline decoration-dotted', className)}
        >
          {sentence.short}
        </LinkButton>
      </TooltipTrigger>
      <TooltipContent>{sentence.full}</TooltipContent>
    </Tooltip>
  )
}

/** An inline text action: shadcn `Button` as a link, sized to its text (44 px tall on touch). */
export function LinkButton({ className, ...props }: ComponentProps<typeof Button>) {
  return (
    <Button
      type="button"
      variant="link"
      className={cn(
        'h-auto p-0 text-left whitespace-normal has-[>svg]:px-0 pointer-coarse:min-h-11',
        className,
      )}
      {...props}
    />
  )
}

// ─── the page's one polite live region ──────────────────────────────────────

/** Wraps the page: one `aria-live="polite"` region, and `useAnnounce()` for every save outcome and fallback move. */
export function Announcer({ children }: { children: ReactNode }) {
  const [msg, setMsg] = useState('')
  const announce = useCallback((m: string) => {
    // Clear first so the same message twice is read twice.
    setMsg('')
    queueMicrotask(() => setMsg(m))
  }, [])
  return (
    <AnnounceContext.Provider value={announce}>
      {children}
      <p className="sr-only" aria-live="polite" role="status" data-testid="router-announcer">
        {msg}
      </p>
    </AnnounceContext.Provider>
  )
}
