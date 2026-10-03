/**
 * F4 — "Which traces burned it?" (plan F4, A2b, A5, A11, A18).
 *
 * Built on the PROPOSED `/finops/top-traces` contract. In live mode the endpoint does
 * not exist yet: a 404 renders an explanatory state, not an error. Links open the
 * existing nasiko UI session/trace view in a new tab.
 */
import { ExternalLink, Info } from 'lucide-react'
import { useRef } from 'react'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { isTopTracesAbsent } from '@/lib/api/detect'
import { TOP_TRACES_LIMIT } from '../api'
import { fmtLatency, fmtLocalDateTime, fmtMoney, fmtTokens } from '@/lib/format'
import { traceLink } from '../links'
import type { TopTracesData } from '../types'
import { PanelEmpty, PanelError, PanelSkeleton } from '@/components/shared/panel'

export function TracesDrawer({
  open,
  onOpenChange,
  scopeLabel,
  data,
  loading,
  error,
  onRetry,
  legacyUiUrl,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  scopeLabel: string
  data: TopTracesData | undefined
  loading: boolean
  error: unknown
  onRetry: () => void
  legacyUiUrl: string | null
}) {
  const titleRef = useRef<HTMLHeadingElement | null>(null)
  // The drawer opens from the URL, not a Radix trigger, so Radix has nowhere to return
  // focus on close. Remember what was focused when it opened and go back there.
  const openerRef = useRef<HTMLElement | null>(null)
  // A route-level 404 means this server predates the proposed endpoint. A 404 whose body is
  // "agent not found" is an access answer for the agent filter, like the dashboard's (A29).
  const missingEndpoint = isTopTracesAbsent(error)
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        className="w-full gap-0 overflow-y-auto sm:max-w-130"
        onOpenAutoFocus={(e) => {
          const active = document.activeElement
          openerRef.current =
            active instanceof HTMLElement && active !== document.body ? active : null
          // Land on the heading so screen readers announce what opened (A11).
          e.preventDefault()
          titleRef.current?.focus()
        }}
        onCloseAutoFocus={(e) => {
          const opener = openerRef.current
          openerRef.current = null
          if (opener?.isConnected) {
            e.preventDefault()
            opener.focus()
          }
        }}
      >
        <SheetHeader>
          <SheetTitle ref={titleRef} tabIndex={-1} className="outline-none">
            Most expensive traces
          </SheetTitle>
          <SheetDescription>{scopeLabel}</SheetDescription>
        </SheetHeader>
        <div className="px-4 pb-6">
          {loading && !data ? (
            <PanelSkeleton height={300} />
          ) : missingEndpoint ? (
            <div
              role="status"
              className="flex gap-2 rounded-md border border-info/30 bg-info/5 p-3 text-sm"
            >
              <Info className="mt-0.5 size-4 shrink-0 text-info" aria-hidden />
              <div>
                <p className="font-medium">Trace drill-down needs a newer nasiko-server</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  This view uses the proposed <code>/finops/top-traces</code> endpoint, which this
                  server doesn't have yet. Set <code>VITE_NASIKO_MOCK=top-traces</code> to preview
                  it with mock data.
                </p>
              </div>
            </div>
          ) : error && !data ? (
            <PanelError error={error} onRetry={onRetry} what="traces" />
          ) : !data?.rows.length ? (
            <PanelEmpty title="No traces in this scope" />
          ) : (
            <ol className="flex flex-col divide-y divide-border">
              {data.rows.map((t) => {
                const href = traceLink(legacyUiUrl, t.session_id, t.trace_id)
                const when = fmtLocalDateTime(t.started_at)
                return (
                  <li
                    key={`${t.trace_id}:${t.agent_name}`}
                    className="flex flex-col gap-1 py-3 text-sm"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="truncate font-medium">{t.agent_name}</span>
                      <span className="font-semibold tabular-nums">{fmtMoney(t.cost_usd)}</span>
                    </div>
                    <div className="flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-muted-foreground">
                      <span>{when}</span>
                      <span>{t.model ?? 'unknown model'}</span>
                      <span className="tabular-nums">
                        {fmtTokens(t.input_tokens + t.output_tokens)} tokens
                      </span>
                      <span className="tabular-nums">{fmtLatency(t.latency_ms)}</span>
                      {t.cost_usd === 0 && t.input_tokens > 0 ? (
                        <span className="text-warning">unpriced</span>
                      ) : null}
                    </div>
                    {href ? (
                      <Button asChild variant="link" size="sm" className="h-auto w-fit p-0 text-xs">
                        <a href={href} target="_blank" rel="noopener noreferrer">
                          Open trace
                          <span className="sr-only">
                            : {t.agent_name}, {when}, {fmtMoney(t.cost_usd)} (opens in a new tab)
                          </span>{' '}
                          <ExternalLink className="size-3" aria-hidden />
                        </a>
                      </Button>
                    ) : null}
                  </li>
                )
              })}
            </ol>
          )}
          {data?.has_more ? (
            <p className="mt-2 text-xs text-muted-foreground">
              Showing the top {TOP_TRACES_LIMIT} by cost.
            </p>
          ) : null}
        </div>
      </SheetContent>
    </Sheet>
  )
}
