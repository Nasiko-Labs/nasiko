/**
 * What's optimizing your tokens (plans/feat-optimization-page.md §11, ledger B12): every mechanism that cuts tokens,
 * what it trims, where it is set and its state now, so nobody has to know that PACMS is a personal setting while
 * caveman and ponytail are per-agent switches. Program names come from the savings data's own
 * `label` where it answers.
 *
 * - History selection: your own setting, on this page (Your settings); tier sizes are a superuser setting.
 * - Smaller prompts, Less code written, Prompt comments: switches on each agent's Settings tab
 *   (P5: this block links there, it never edits them). Counts (workspace-wide) and savings come from `/finops/savings`
 *   (main's real endpoint, synced from nasiko-cloud-rs `c8052fed` `savings.rs`); a server without it keeps the rows and links (R2B) and says what will show.
 */
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import type { ReactNode } from 'react'
import { Panel } from '@nasiko/ui/composites/panel'
import { Button } from '@nasiko/ui/primitives/button'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { useSavings } from '@nasiko/features/tokenops/api'
import type { ResolvedWindow } from '@nasiko/features/tokenops/window'
import { isEndpointAbsent } from '@nasiko/data-access/detect'
import { budgetQuery, strategyQuery } from '../api'
import { copy } from '../copy'
import { Codename, type CodenameKey } from './Codename'
import { tokensShort } from '../lead'

const NO_FILTERS = {}

export function Mechanisms({
  win,
  period,
  superuser,
  enabled,
}: {
  win: ResolvedWindow
  period: string
  superuser: boolean
  enabled: boolean
}) {
  const strategy = useQuery(strategyQuery)
  const budget = useQuery(budgetQuery)
  const savings = useSavings(win, NO_FILTERS, 'total', enabled)
  const m = copy.mechanisms
  // `useSavings` keeps the previous window's data while a new one loads (keepPreviousData): never show that under this
  // window's period, and never let it hide this window's failure.
  const data = savings.isPlaceholderData ? undefined : savings.data
  const absent = savings.isError && isEndpointAbsent(savings.error)
  const failed = savings.isError && !absent
  const program = (id: string) => data?.by_program.find((p) => p.program === id)
  const cov = data?.coverage

  // A skeleton only while a read is pending; a failed or absent preference read shows no state (R2C: never a forever
  // skeleton), since Your settings below has its own error and Retry.
  const historyState =
    strategy.isPending || budget.isPending
      ? undefined
      : !strategy.data || !budget.data
        ? null
        : strategy.data.enabled === false
          ? m.off
          : `${copy.strategies[strategy.data.strategy].name} · ${copy.level(budget.data.level)}`

  const programRow = (id: 'caveman' | 'ponytail', fallback: string) => {
    const p = program(id)
    return {
      name: p ? p.label : fallback,
      saved:
        p && p.saved_tokens > 0
          ? m.saved(tokensShort(p.saved_tokens), p.basis !== 'measured')
          : null,
    }
  }
  const caveman = programRow('caveman', m.caveman)
  const ponytail = programRow('ponytail', m.ponytail)
  // Loading: a skeleton; absent or failed: no state (the footnote says why); otherwise the count. The server counts
  // every live agent in the workspace (savings.rs `coverage`), not the viewer's, so the line says so.
  const count = (on: number | undefined, total?: number) =>
    !data && !savings.isError && enabled
      ? undefined
      : on === undefined
        ? null
        : m.agentsOn(on, total)

  return (
    <Panel title={m.title} subtitle={period} labelledBy="optimization-mechanisms-h">
      <ul className="m-0 grid list-none gap-3 p-0 md:grid-cols-2 2xl:grid-cols-4">
        <Row
          name={m.history}
          codename="pacms"
          trims={m.historyTrims}
          where={
            <>
              <Link
                to="/optimization"
                search={(prev) => prev}
                hash="settings"
                className="underline underline-offset-4 hover:text-primary-text"
              >
                {m.historyWhere}
              </Link>
              {superuser ? (
                <>
                  {' · '}
                  {m.tiersNote}{' '}
                  <Link
                    to="/settings/optimization-tiers"
                    className="underline underline-offset-4 hover:text-primary-text"
                  >
                    {m.tiersWhere}
                  </Link>
                </>
              ) : null}
            </>
          }
          state={historyState}
          saved={null}
        />
        <Row
          name={caveman.name}
          trims={m.cavemanTrims}
          where={<AgentsLink label={m.cavemanWhere} />}
          state={count(cov?.agents_with_compress_enabled, cov?.agents_total)}
          saved={caveman.saved}
        />
        <Row
          name={ponytail.name}
          trims={m.ponytailTrims}
          where={<AgentsLink label={m.ponytailWhere} />}
          state={count(cov?.agents_with_minimal_code_enabled)}
          saved={ponytail.saved}
        />
        <Row
          name={m.comments}
          trims={m.commentsTrims}
          where={<AgentsLink label={m.commentsWhere} />}
          state={count(cov?.agents_with_prompt_comments)}
          saved={null}
        />
      </ul>
      {absent ? (
        <p className="m-0 text-xs text-muted-foreground">{m.absent}</p>
      ) : failed ? (
        <p className="m-0 text-xs text-muted-foreground">
          {m.failed}{' '}
          <Button
            variant="link"
            size="sm"
            className="h-auto p-0 text-xs pointer-coarse:min-h-11"
            onClick={() => void savings.refetch()}
          >
            {copy.retry}
          </Button>
        </p>
      ) : null}
    </Panel>
  )
}

/** Per-agent switches live on each agent's Settings tab; the link opens your agents. */
function AgentsLink({ label }: { label: string }) {
  return (
    <>
      {label}:{' '}
      <Link to="/agents/mine" className="underline underline-offset-4 hover:text-primary-text">
        {copy.mechanisms.yourAgents}
      </Link>
    </>
  )
}

function Row({
  name,
  codename,
  trims,
  where,
  state,
  saved,
}: {
  name: string
  /** The team name this row introduces in a tooltip (B12). */
  codename?: CodenameKey
  trims: string
  where: ReactNode
  /** undefined while loading; null when there is nothing to show. */
  state: string | null | undefined
  saved: string | null
}) {
  return (
    // A tile per mechanism: what it trims and where it is set on the left, its state now on the right.
    <li className="flex flex-col justify-between gap-3 rounded-lg border bg-muted/20 p-4 transition-colors hover:border-foreground/20">
      <div className="flex flex-col gap-0.5">
        <span className="text-sm font-medium">
          {codename ? <Codename label={name} name={codename} /> : name}
        </span>
        <span className="text-xs text-muted-foreground">{trims}</span>
        <span className="text-xs text-muted-foreground">
          {copy.mechanisms.setIn} {where}
        </span>
      </div>
      {/* The state at the tile's foot, so every tile's figure sits on one line. */}
      <div className="flex flex-col gap-0.5 border-t border-border pt-3">
        {state === undefined ? (
          <Skeleton aria-hidden className="h-4 w-28" />
        ) : state ? (
          <span className="text-sm tabular-nums">{state}</span>
        ) : null}
        {saved ? <span className="text-xs text-muted-foreground tabular-nums">{saved}</span> : null}
      </div>
    </li>
  )
}
