/**
 * Small shared pieces for the agent pages: status badge (copy controls live in
 * components/shared/copy-button), error note, first-run steps, monogram, "Learn more" link,
 * and `Section` (the shared `Panel`, named by its heading).
 */
import { Link } from '@tanstack/react-router'
import { ChevronRight, ExternalLink, MessagesSquare, RotateCw } from 'lucide-react'
import { useId, useState, type ReactNode } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { CopyButton } from '@/components/shared/copy-button'
import { Panel } from '@/components/shared/panel'
import { SectionNav } from '@/components/shared/section-nav'
import { cn } from '@/lib/utils'
import { firstRunCommands } from '../format'
import { copy, DOCS, errorCopy, type ErrorContext } from '../copy'
import { safeHttpsUrl } from '../normalize'
import { STATUS, statusTooltip, type DisplayStatus } from '../status'

/** Text plus colour, never a colour dot alone (plan §6.1). */
export function StatusBadge({
  display,
  raw,
  className,
}: {
  display: DisplayStatus
  raw?: string | null
  className?: string
}) {
  const info = STATUS[display]
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Badge variant={info.tone} className={className} data-status={display}>
          {info.label}
        </Badge>
      </TooltipTrigger>
      <TooltipContent>{statusTooltip(display, raw)}</TooltipContent>
    </Tooltip>
  )
}

/** A command line with its copy button; the CLI note reminds which cluster it runs against. */
export function CliCommand({ command, note = true }: { command: string; note?: boolean }) {
  return (
    <div className="space-y-1">
      <CopyButton text={command} label={copy.copyCommand(command)} showText />
      {note ? <p className="text-xs text-muted-foreground">{copy.cliNote}</p> : null}
    </div>
  )
}

/** Our copy first; the server's own text behind "Details" (plan §6.4). `onRetry` for a failed read. */
export function ErrorNote({
  error,
  context,
  className,
  onRetry,
}: {
  error: unknown
  context?: ErrorContext
  className?: string
  onRetry?: () => void
}) {
  const { message, detail } = errorCopy(error, context)
  return (
    <div role="alert" className={cn('text-sm text-destructive', className)}>
      <span>{message}</span>
      {onRetry ? (
        <Button size="sm" variant="outline" className="ml-2 h-7" onClick={onRetry}>
          <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
        </Button>
      ) : null}
      {detail ? (
        <Collapsible className="mt-1 text-xs text-muted-foreground">
          <CollapsibleTrigger asChild>
            <Button
              variant="link"
              size="sm"
              className="group h-auto gap-0.5 p-0 text-xs text-muted-foreground"
            >
              <ChevronRight
                className="size-3 transition-transform group-data-[state=open]:rotate-90"
                aria-hidden
              />
              {copy.details}
            </Button>
          </CollapsibleTrigger>
          {/* Mounted while closed (hidden), like the native disclosure it replaced: the server's text stays in the document. */}
          <CollapsibleContent forceMount className="whitespace-pre-wrap data-[state=closed]:hidden">
            {detail}
          </CollapsibleContent>
        </Collapsible>
      ) : null}
    </div>
  )
}

export function FirstRunSteps({
  origin = globalThis.location?.origin ?? '<server-url>',
}: {
  origin?: string
}) {
  const cmds = firstRunCommands(origin)
  return (
    <div className="space-y-3 text-sm">
      <p className="text-muted-foreground">{copy.firstRunIntro}</p>
      <ol className="space-y-2">
        {cmds.map((c, i) => (
          <li key={c} className="flex flex-wrap items-center gap-2">
            <span className="text-muted-foreground">
              {i + 1}. {copy.firstRunSteps[i]}
            </span>
            <CopyButton text={c} label={copy.copyCommand(c)} showText />
          </li>
        ))}
      </ol>
      <LearnMore href="lifecycle" />
    </div>
  )
}

export function LearnMore({ href }: { href: keyof typeof DOCS }) {
  return (
    <a
      href={DOCS[href]}
      target="_blank"
      rel="noreferrer"
      className="inline-flex items-center gap-1 text-xs text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
    >
      {copy.learnMore} <ExternalLink className="size-3" aria-hidden />
    </a>
  )
}

/**
 * Icon (https only, no referrer: no mixed content, and a plain-http tracking host is refused)
 * with a monogram fallback when it's missing or fails.
 */
export function AgentMark({
  name,
  iconUrl,
  size = 40,
}: {
  name: string
  iconUrl?: string | null
  size?: number
}) {
  const [broken, setBroken] = useState(false)
  const src = safeHttpsUrl(iconUrl)
  const letters =
    name
      .split(/[\s_-]+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((w) => w.charAt(0).toUpperCase())
      .join('') || '?'
  const style = { width: size, height: size }
  if (src && !broken) {
    return (
      <img
        src={src}
        alt=""
        referrerPolicy="no-referrer"
        onError={() => setBroken(true)}
        style={style}
        className="shrink-0 rounded-md border border-border object-cover"
      />
    )
  }
  return (
    <span
      aria-hidden
      style={style}
      className="inline-flex shrink-0 items-center justify-center rounded-md bg-primary/10 text-sm font-semibold text-primary-text"
    >
      {letters}
    </span>
  )
}

export function Section({
  title,
  subtitle,
  action,
  children,
  className,
}: {
  title: string
  subtitle?: ReactNode
  action?: ReactNode
  children: ReactNode
  className?: string
}) {
  const id = useId()
  return (
    <Panel title={title} subtitle={subtitle} labelledBy={id} actions={action} className={className}>
      {children}
    </Panel>
  )
}

/** In-app link to an agent's detail page, by UUID (AgentLink resolves names and falls back to text). */
export function AgentLinkTo({
  id,
  children,
  className,
}: {
  id: string
  children: ReactNode
  className?: string
}) {
  return (
    <Link
      to="/agents/$agentId"
      params={{ agentId: id }}
      search={{}}
      className={cn('underline-offset-4 hover:underline', className)}
    >
      {children}
    </Link>
  )
}

/** Opens Chat with this agent (plan §9). Only running A2A agents; harnesses never run as containers. */
export function TryItLink({
  id,
  name,
  display,
  harness,
  size = 'sm',
  variant = 'outline',
}: {
  id: string
  name: string
  display: DisplayStatus
  harness: boolean
  size?: 'sm' | 'xs'
  variant?: 'outline' | 'default'
}) {
  if (harness || display !== 'running') return null
  return (
    <Button asChild size={size} variant={variant} className="pointer-coarse:min-h-11">
      <Link to="/chat" search={{ agent: id }} title={copy.tryItTitle(name)}>
        <MessagesSquare aria-hidden /> {copy.tryIt}
      </Link>
    </Button>
  )
}

const SECTIONS = [
  { to: '/agents', label: copy.sections.all },
  { to: '/agents/mine', label: copy.sections.mine },
  { to: '/builds', label: copy.sections.builds },
] as const

/** All agents · Your agents · Builds (the Builds page uses it too); "Deploy an agent" stays the header's action. */
export function AgentsNav({ current }: { current: (typeof SECTIONS)[number]['to'] }) {
  return <SectionNav label={copy.sections.label} sections={SECTIONS} current={current} />
}
