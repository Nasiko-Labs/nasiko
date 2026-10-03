/**
 * Span panel: Summary (status, timing, tokens, cost, ids), Prompt & response (always
 * rendered as TEXT, never HTML), Attributes (grouped by namespace, filterable, click to
 * copy). Per-span cost only exists on SpanDetail, so it is fetched on selection.
 * Patterns: Braintrust's span pane (previous/next span, key-value summary, collapsible
 * Input/Output with a view toggle) and Axiom's (span id with copy, tab counts, raw fields
 * grouped and filterable).
 */
import { ChevronDown, ChevronRight, ChevronUp, Copy, X } from 'lucide-react'
import { useMemo, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { SearchInput } from '@/components/shared/search-input'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Toggle } from '@/components/ui/toggle'
import { copy } from '@/features/observability/copy'
import {
  CLASS_LABEL,
  flattenAttributes,
  isError,
  statusText,
  TRACE_TOTAL_SPAN,
  type FlatSpan,
} from '@/features/observability/spans'
import { CONTENT_PREVIEW_CHARS } from '@/features/observability/tuning'
import { ErrorState } from '@/features/observability/StateCard'
import type { ContentField } from '@/features/observability/types'
import { fmtLatency, fmtMoney, fmtTokens } from '@/lib/format'
import { useCopy } from '@/lib/useCopy'
import { cn } from '@/lib/utils'
import { useSpanDetail } from './api'

const TABS = ['summary', 'content', 'attributes'] as const
export type SpanTab = (typeof TABS)[number]
type Tab = SpanTab
const TAB_LABEL: Record<Tab, string> = {
  summary: 'Summary',
  content: 'Prompt & response',
  attributes: 'Attributes',
}

/**
 * The panel stays mounted while stepping through spans (so Previous/Next keep focus); the
 * span-local parts (content views, attribute filter, copy feedback) are keyed by span id
 * and reset per span. The selected tab lives in the page.
 */
export function SpanPanel({
  traceId,
  span,
  tab,
  onTabChange,
  parentName,
  position,
  onPrev,
  onNext,
  onClose,
}: {
  traceId: string
  span: FlatSpan
  tab: Tab
  onTabChange: (tab: Tab) => void
  parentName?: string
  /** 1-based position in tree order, for "3 of 18". */
  position?: { index: number; of: number }
  onPrev?: () => void
  onNext?: () => void
  onClose?: () => void
}) {
  const setTab = onTabChange
  const detail = useSpanDetail(traceId, span.node.span_id)
  const d = detail.data
  const attrs = useMemo(() => flattenAttributes(d?.attributes), [d?.attributes])
  const attrCount = Object.keys(attrs).length
  const atStart = !position || position.index <= 1
  const atEnd = !position || position.index >= position.of

  return (
    <section
      aria-labelledby="span-panel-title"
      className="flex min-w-0 flex-col gap-3 rounded-lg border border-border bg-card p-4"
    >
      <header className="flex flex-col gap-1">
        <div className="flex items-center justify-between gap-2">
          <p className="text-xs font-medium text-muted-foreground">
            Span
            {position ? (
              <span className="font-normal tabular-nums">
                {' '}
                · {position.index} of {position.of}
              </span>
            ) : null}
          </p>
          <div className="flex items-center gap-0.5">
            {/* aria-disabled (not disabled) so focus stays put at either end of the list. */}
            {onPrev ? (
              <Button
                variant="ghost"
                size="icon"
                className="size-10 aria-disabled:opacity-40"
                onClick={() => {
                  if (!atStart) onPrev()
                }}
                aria-disabled={atStart}
                aria-label={copy.prevSpan}
              >
                <ChevronUp className="size-4" aria-hidden />
              </Button>
            ) : null}
            {onNext ? (
              <Button
                variant="ghost"
                size="icon"
                className="size-10 aria-disabled:opacity-40"
                onClick={() => {
                  if (!atEnd) onNext()
                }}
                aria-disabled={atEnd}
                aria-label={copy.nextSpan}
              >
                <ChevronDown className="size-4" aria-hidden />
              </Button>
            ) : null}
            {onClose ? (
              <Button
                variant="ghost"
                size="icon"
                className="size-10"
                onClick={onClose}
                aria-label="Close span panel"
              >
                <X className="size-4" aria-hidden />
              </Button>
            ) : null}
          </div>
        </div>
        <h2
          id="span-panel-title"
          className="truncate font-mono text-sm font-semibold"
          title={span.node.name}
        >
          {span.node.name}
        </h2>
        <p className="text-xs text-muted-foreground">
          {CLASS_LABEL[span.cls]}
          {span.node.model ? ` · ${span.node.model}` : ''}
        </p>
        <SpanIdRow key={span.node.span_id} id={span.node.span_id} />
      </header>

      {/* Radix Tabs: arrows, Home and End move and select (automatic activation); only the
          selected panel is mounted. The selected tab lives in the page. */}
      <Tabs value={tab} onValueChange={(v) => setTab(v as Tab)} className="gap-3">
        <TabsList
          variant="line"
          aria-label="Span details"
          className="h-auto w-full justify-start gap-0.5 overflow-x-auto rounded-none border-b border-border p-0"
        >
          {TABS.map((t) => (
            <TabsTrigger
              key={t}
              value={t}
              className="min-h-10 flex-none px-1.5 font-normal data-[state=active]:font-medium"
            >
              {TAB_LABEL[t]}
              {t === 'attributes' && d ? (
                <Badge
                  variant="muted"
                  aria-hidden
                  className="rounded px-1.5 py-0 text-2xs tabular-nums"
                >
                  {attrCount}
                </Badge>
              ) : null}
            </TabsTrigger>
          ))}
        </TabsList>

        <TabsContent key={span.node.span_id} value={tab} className="min-w-0">
          {tab === 'summary' ? (
            <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm">
              <dt className="text-muted-foreground">Status</dt>
              <dd className={isError(span.node) ? 'text-destructive' : ''}>
                {statusText(span.node)}
                {d?.status_message ? ` · ${d.status_message}` : ''}
              </dd>
              <dt className="text-muted-foreground">Start</dt>
              <dd className="tabular-nums">
                +{fmtLatency(span.startMs)}{' '}
                <span className="text-muted-foreground">from trace start</span>
              </dd>
              <dt className="text-muted-foreground">Duration</dt>
              <dd className="tabular-nums">{fmtLatency(span.durationMs)}</dd>
              <dt className="text-muted-foreground">Tokens</dt>
              <dd className="tabular-nums">
                {fmtTokens(span.node.input_tokens)} in · {fmtTokens(span.node.output_tokens)} out
                {span.node.cache_read_tokens
                  ? ` · ${fmtTokens(span.node.cache_read_tokens)} cached`
                  : ''}
              </dd>
              <dt className="text-muted-foreground">Cost</dt>
              <dd className="tabular-nums">
                {detail.isPending ? (
                  <span className="text-muted-foreground">Loading…</span>
                ) : detail.isError ? (
                  <span className="text-muted-foreground">unavailable</span>
                ) : (
                  <span title={d ? `$${d.cost_summary.total.cost}` : undefined}>
                    {fmtMoney(d?.cost_summary.total.cost)}
                  </span>
                )}
              </dd>
              <dt className="text-muted-foreground">Kind</dt>
              <dd>
                {CLASS_LABEL[span.cls]}{' '}
                <span className="text-muted-foreground">({span.node.span_kind})</span>
              </dd>
              {span.node.model ? (
                <>
                  <dt className="text-muted-foreground">Model</dt>
                  <dd className="font-mono text-xs leading-5">{span.node.model}</dd>
                </>
              ) : null}
              {parentName ? (
                <>
                  <dt className="text-muted-foreground">Parent</dt>
                  <dd className="truncate font-mono text-xs leading-5" title={parentName}>
                    {parentName}
                  </dd>
                </>
              ) : null}
              {span.node.name === TRACE_TOTAL_SPAN ? (
                <dd className="col-span-2 text-xs text-muted-foreground">
                  This span carries the whole turn's usage; it isn't added to span totals.
                </dd>
              ) : null}
            </dl>
          ) : detail.isPending ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : detail.isError ? (
            <ErrorState error={detail.error} onRetry={() => void detail.refetch()} />
          ) : d && tab === 'content' ? (
            <div className="flex flex-col gap-2">
              <ContentSection label="Input" field={d.input} />
              <ContentSection label="Output" field={d.output} />
            </div>
          ) : (
            <Attributes attrs={attrs} />
          )}
        </TabsContent>
      </Tabs>
    </section>
  )
}

function SpanIdRow({ id }: { id: string }) {
  const [copied, run] = useCopy(id)
  return (
    <div className="flex items-center gap-1 text-xs text-muted-foreground">
      <span className="truncate font-mono">{id}</span>
      <Button
        variant="ghost"
        size="icon-sm"
        onClick={() => void run()}
        aria-label={copy.copySpanId}
        className="-m-1 size-7 hover:text-foreground"
      >
        <Copy className="size-3.5" aria-hidden />
      </Button>
      <span role="status" className="text-success">
        {copied === 'copied' ? copy.copied : ''}
      </span>
    </div>
  )
}

function prettyJson(value: string): string | null {
  try {
    const parsed: unknown = JSON.parse(value)
    return typeof parsed === 'object' && parsed !== null ? JSON.stringify(parsed, null, 2) : null
  } catch {
    return null
  }
}

/** Collapsible Input/Output with a Text / JSON view toggle (JSON only when it parses). */
function ContentSection({ label, field }: { label: string; field: ContentField }) {
  const [open, setOpen] = useState(true)
  const [view, setView] = useState<'text' | 'json'>('text')
  const [more, setMore] = useState(false)
  const value = field.value ?? ''
  const json = useMemo(() => prettyJson(value), [value])
  const shown = view === 'json' && json ? json : value
  const long = shown.length > CONTENT_PREVIEW_CHARS
  return (
    <Collapsible
      open={open}
      onOpenChange={setOpen}
      className="border-b border-border pb-2 last:border-b-0"
    >
      <div className="flex items-center justify-between gap-2">
        <CollapsibleTrigger asChild>
          <Button variant="ghost" size="sm" className="-ml-1 h-auto min-h-8 gap-1 px-1">
            <ChevronRight
              className={cn(
                'size-3.5 text-muted-foreground transition-transform',
                open && 'rotate-90',
              )}
              aria-hidden
            />
            {label}
          </Button>
        </CollapsibleTrigger>
        {open && value && json ? (
          <div
            role="group"
            aria-label={`${label} view`}
            className="flex rounded-md border border-border p-0.5 text-xs"
          >
            {(['text', 'json'] as const).map((v) => (
              <Toggle
                key={v}
                size="sm"
                pressed={view === v}
                onClick={() => setView(v)}
                className="h-6 min-w-0 rounded px-2 text-xs font-normal text-muted-foreground data-[state=on]:font-medium"
              >
                {v === 'text' ? 'Text' : 'JSON'}
              </Toggle>
            ))}
          </div>
        ) : null}
      </div>
      <CollapsibleContent className="mt-1">
        {!value ? (
          <p className="text-sm text-muted-foreground">{copy.captureOff}</p>
        ) : (
          <>
            {/* Text only: model output is untrusted and never rendered as HTML. */}
            <pre className="max-h-80 overflow-auto rounded-md bg-muted/60 p-2 text-code leading-5 break-words whitespace-pre-wrap">
              {long && !more ? `${shown.slice(0, CONTENT_PREVIEW_CHARS)}…` : shown}
            </pre>
            {long ? (
              <Button
                variant="link"
                size="sm"
                className="h-auto p-0"
                onClick={() => setMore((m) => !m)}
              >
                {more ? 'Show less' : 'Show all'}
              </Button>
            ) : null}
          </>
        )}
      </CollapsibleContent>
    </Collapsible>
  )
}

/** Keys without a namespace (no dot, or a leading dot) share this group; it sorts last. */
const NO_NAMESPACE = '(no namespace)'
/** `gen_ai.usage.input_tokens` → `gen_ai`. */
const namespaceOf = (key: string) =>
  key.indexOf('.') > 0 ? key.slice(0, key.indexOf('.')) : NO_NAMESPACE
/** The key shown under its group heading (the full key if nothing would remain). */
const shortKey = (key: string, ns: string) =>
  ns !== NO_NAMESPACE && key.length > ns.length + 1 ? key.slice(ns.length + 1) : key

function Attributes({ attrs }: { attrs: Record<string, unknown> }) {
  const [q, setQ] = useState('')
  // Which key the one copy state belongs to.
  const [copiedKey, setCopiedKey] = useState<string | null>(null)
  const [copyState, copyValue] = useCopy()
  const copied = copyState === 'copied' ? copiedKey : null
  const [closed, setClosed] = useState<ReadonlySet<string>>(new Set())
  const needle = q.toLowerCase()
  const groups = useMemo(() => {
    const out = new Map<string, [string, unknown][]>()
    for (const [k, v] of Object.entries(attrs)) {
      if (needle && !k.toLowerCase().includes(needle) && !String(v).toLowerCase().includes(needle))
        continue
      const ns = namespaceOf(k)
      out.set(ns, [...(out.get(ns) ?? []), [k, v]])
    }
    return [...out.entries()].sort(([a], [b]) =>
      a === NO_NAMESPACE ? 1 : b === NO_NAMESPACE ? -1 : a.localeCompare(b),
    )
  }, [attrs, needle])
  const toggle = (ns: string) =>
    setClosed((prev) => {
      const n = new Set(prev)
      if (n.has(ns)) n.delete(ns)
      else n.add(ns)
      return n
    })
  return (
    <div className="flex flex-col gap-2">
      <SearchInput
        value={q}
        onChange={(e) => setQ(e.target.value)}
        placeholder="Filter attributes"
        aria-label="Filter attributes"
        className="h-8 max-w-none"
      />
      <span role="status" className="sr-only">
        {copied ? `${copy.copied} ${copied}` : ''}
      </span>
      {groups.length ? (
        groups.map(([ns, entries]) => {
          const open = !closed.has(ns) || !!needle
          return (
            <Collapsible key={ns} open={open} onOpenChange={() => toggle(ns)} disabled={!!needle}>
              {/* While filtering every group is open, so the toggle is disabled rather than silent. */}
              <CollapsibleTrigger asChild>
                <Button
                  variant="ghost"
                  size="sm"
                  className="-ml-1 h-auto min-h-8 gap-1 px-1 font-mono text-xs disabled:opacity-100"
                >
                  <ChevronRight
                    className={cn(
                      'size-3.5 text-muted-foreground transition-transform',
                      open && 'rotate-90',
                    )}
                    aria-hidden
                  />
                  {ns}{' '}
                  <span className="font-sans font-normal text-muted-foreground">
                    {entries.length}
                  </span>
                </Button>
              </CollapsibleTrigger>
              <CollapsibleContent asChild>
                <dl className="flex flex-col divide-y divide-border text-sm">
                  {entries.map(([k, v]) => (
                    <div key={k} className="flex items-start gap-2 py-1.5">
                      {/* The group heading carries the namespace; the full key is in the tooltip. */}
                      <dt
                        className="w-2/5 shrink-0 font-mono text-xs break-words text-muted-foreground"
                        title={k}
                      >
                        {shortKey(k, ns)}
                      </dt>
                      <dd className="min-w-0 flex-1 font-mono text-xs break-all">
                        {typeof v === 'string' ? v : JSON.stringify(v)}
                      </dd>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        onClick={() => {
                          setCopiedKey(k)
                          void copyValue(typeof v === 'string' ? v : JSON.stringify(v))
                        }}
                        aria-label={`Copy ${k}`}
                        className="-m-2 text-muted-foreground hover:text-foreground"
                      >
                        <Copy className="size-3.5" aria-hidden />
                      </Button>
                      {copied === k ? (
                        <span aria-hidden className="text-xs text-success">
                          {copy.copied}
                        </span>
                      ) : null}
                    </div>
                  ))}
                </dl>
              </CollapsibleContent>
            </Collapsible>
          )
        })
      ) : (
        <p className="text-sm text-muted-foreground">No attributes{q ? ' match' : ''}.</p>
      )}
    </div>
  )
}
