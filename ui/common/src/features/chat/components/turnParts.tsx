/**
 * Pieces shared by the direct and routed turn views (plan §7.4; v1b §5.5-§5.7): the usage chip,
 * Copy, and the error notice with its one primary action.
 */
import { Check, ChevronRight, CircleAlert, Copy } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { fmtCostPerOp, fmtLatency, fmtTokens } from '@/lib/format'
import { useCopy } from '@/lib/useCopy'
import { cn } from '@/lib/utils'
import { copy, errorCopy } from '../copy'
import { docAnchor } from '../copyDetails'
import type { ChatError } from '../errors'
import type { TurnUsage } from '../format'
import { DISCLOSE, TOUCH } from './turnStyles'

export function UsageChip({ usage }: { usage: TurnUsage }) {
  const parts = [
    usage.tokens !== null
      ? copy.tokens(`${usage.estimated ? '~' : ''}${fmtTokens(usage.tokens)}`)
      : null,
    usage.durationMs !== null ? fmtLatency(usage.durationMs) : null,
    usage.cost !== null && usage.cost > 0
      ? `${usage.estimated ? '~' : ''}${fmtCostPerOp(usage.cost)}`
      : null,
  ].filter(Boolean)
  if (!parts.length) return null
  const detail = [
    usage.inputTokens !== null ? `${usage.inputTokens} in` : null,
    usage.outputTokens !== null ? `${usage.outputTokens} out` : null,
    usage.model,
  ]
    .filter(Boolean)
    .join(' · ')
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- focusable only so keyboard users can open the detail tooltip; it has no action, so no button role
          tabIndex={detail ? 0 : undefined}
          className="rounded-md border border-border px-1.5 py-0.5 text-xs text-muted-foreground tabular-nums"
          data-testid="usage-chip"
        >
          {parts.join(' · ')}
        </span>
      </TooltipTrigger>
      {detail ? <TooltipContent>{detail}</TooltipContent> : null}
    </Tooltip>
  )
}

/** Copy on a reply's footer. */
export function CopyReply({ text }: { text: string }) {
  return <CopyText text={text} label={copy.copyReply} size="xs" variant="ghost" />
}

export type NoticeAction = { label: string; onClick: () => void; primary?: boolean }

/** `details`: routed notices add Copy details (phase, ids, codes) and their doc anchor (v1b §5.6). */
export function ErrorNotice({
  error,
  actions,
  links,
  details,
}: {
  error: ChatError
  actions: NoticeAction[]
  links?: React.ReactNode
  details?: string
}) {
  const c = errorCopy[error.key]
  const detail = [
    error.serverDetail ? copy.serverSaid(error.serverDetail) : null,
    error.status ? `HTTP ${error.status}` : null,
    error.rpcCode ? `code ${error.rpcCode}` : null,
  ]
    .filter(Boolean)
    .join(' · ')
  return (
    <div
      className="space-y-1.5 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-sm"
      data-testid="error-notice"
    >
      <div className="flex items-start gap-2">
        <CircleAlert className="mt-0.5 size-4 text-destructive" aria-hidden />
        <div>
          <div className="font-medium">{c.problem}</div>
          <div className="text-muted-foreground">
            {c.cause} {c.action}
          </div>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {actions.map((a) => (
          <Button
            key={a.label}
            size="sm"
            className={TOUCH}
            variant={a.primary ? 'default' : 'outline'}
            onClick={a.onClick}
          >
            {a.label}
          </Button>
        ))}
        {links}
        {details ? <CopyText text={details} label={copy.copyDetails} /> : null}
      </div>
      {details ? <p className="text-xs text-muted-foreground">{docAnchor(error.key)}</p> : null}
      {detail ? (
        <Collapsible className="text-xs text-muted-foreground">
          <CollapsibleTrigger asChild>
            <Button variant="ghost" size="xs" className={cn(DISCLOSE, TOUCH)}>
              <ChevronRight aria-hidden />
              {copy.details}
            </Button>
          </CollapsibleTrigger>
          {/* Mounted while closed (hidden), as the native details element it replaces was: the text is findable once opened, never refetched. */}
          <CollapsibleContent forceMount className="mt-1 flex items-center gap-2">
            <span className="break-all">{detail}</span>
            <CopyReply text={detail} />
          </CollapsibleContent>
        </Collapsible>
      ) : null}
    </div>
  )
}

/** A Copy button for any text, labelled for what it copies (Copy on a reply, Copy details on a notice). */
export function CopyText({
  text,
  label,
  size = 'sm',
  variant = 'outline',
}: {
  text: string
  label: string
  size?: 'xs' | 'sm'
  variant?: 'ghost' | 'outline'
}) {
  const [state, onCopy] = useCopy(text)
  return (
    <Button size={size} variant={variant} className={TOUCH} onClick={() => void onCopy()}>
      {state === 'copied' ? <Check aria-hidden /> : <Copy aria-hidden />}{' '}
      {state === 'copied' ? copy.copied : state === 'failed' ? copy.copyFailed : label}
    </Button>
  )
}
