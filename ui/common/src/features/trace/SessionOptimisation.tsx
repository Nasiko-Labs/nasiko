/**
 * What optimisation did to this conversation.
 *
 * Two different facts, and the distinction matters: the **setting** that governed how much history
 * each message carried, and the **measured** tokens trimmed out of this session's calls. The first
 * is always knowable; the second only exists once a layer that records its savings has run.
 *
 * Shown on the session page because savings recur per turn — a long conversation compounds them in
 * a way a per-call view understates, and this is where someone looks when they suspect optimisation
 * changed an answer.
 */
import { Scissors } from 'lucide-react'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useChatContext } from '@/features/settings/api'
import { useSessionSavings } from '@/features/tokenops/api'
import { fmtMoney, fmtPct, fmtTokens } from '@/lib/format'

/** How the history strategies read to someone who has not read the design doc. */
const STRATEGY_LABEL: Record<string, string> = {
  pacms: 'relevance-picked',
  topk: 'closest matches',
  lastk: 'most recent',
}

export function SessionOptimisation({ sessionId }: { sessionId: string }) {
  const ctx = useChatContext()
  const savings = useSessionSavings(sessionId)

  const strategy = ctx.data?.strategy
  const budget = ctx.data?.level
  const row = savings.data?.by_session?.[0]
  const saved = row?.saved_tokens ?? 0

  // Nothing to say: no setting readable (an older server has no /api/me routes) and nothing
  // measured. A row that says neither is noise on a page that is already dense.
  if (!strategy && saved === 0) return null

  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
      <span className="flex items-center gap-1.5">
        <Scissors className="size-3.5" aria-hidden />
        <span className="font-medium text-foreground">Context optimisation</span>
      </span>

      {strategy ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <span>
              History: {STRATEGY_LABEL[strategy] ?? strategy}
              {budget ? <> · {budget} budget</> : null}
            </span>
          </TooltipTrigger>
          <TooltipContent className="max-w-xs">
            Each message carries a slice of this conversation rather than all of it. The budget sets
            how big that slice is. Change both in Settings → Chat context.
          </TooltipContent>
        </Tooltip>
      ) : null}

      {saved > 0 ? (
        <span>
          <span className="font-medium text-success">{fmtTokens(saved)} tokens</span> trimmed from
          this session
          {row?.token_reduction_pct == null ? null : <> ({fmtPct(row.token_reduction_pct)})</>}
          {row && row.saved_cost_usd > 0 ? <> · {fmtMoney(row.saved_cost_usd)} saved</> : null}
        </span>
      ) : (
        // Said plainly rather than left blank: "no savings recorded" and "optimisation is off" are
        // different situations, and only one of them is a reason to go and change a setting.
        <span>no trimming recorded on this session&apos;s calls</span>
      )}
    </div>
  )
}
