/**
 * Harnesses: the viewer's own coding-harness usage (plans/feat-harness-org-view.md). This is the core (OSS) page; an
 * edition layer that fills the `harnessOrgScope` slot renders its own org-level page instead (the route decides).
 *
 *   viewer (users/me, or the /api/me claims) → the landing request (no params: the server answers "self")
 *   usage endpoint ──200──▶ Individual (full form)
 *                  ──coded 404──▶ "Not found or not visible"
 *                  ──bare 404──▶ live fallback: the viewer's own Individual (liveIndividual.ts)
 *
 * All state lives in the URL. The harness chip and Compare replace history. At most one status line.
 */
import { SquareTerminal } from 'lucide-react'
import { EmptyState } from '@/components/shared/state-card'
import { useUsersMe } from './api'
import { copy } from './copy'
import { useHarnessWindow, useUsageLevel, type SetSearch } from './level'
import { useLiveIndividual } from './liveIndividual'
import type { HarnessesSearch } from './search'
import {
  HarnessHeader,
  LevelLoading,
  LiveFallbackLevel,
  UsageError,
  UserLevel,
  ViewerError,
} from './components/PageParts'

export function HarnessesPage({
  search,
  setSearch,
}: {
  search: HarnessesSearch
  setSearch: SetSearch
}) {
  const { today, from, win, days, compare } = useHarnessWindow(search)
  const me = useUsersMe(undefined)
  // The landing request: the server scopes it to the viewer.
  const req = { ...win.params, compare }
  const level = useUsageLevel(me.data?.id, req, win.key, !!me.data)
  const { usage, res, absent } = level
  const live = useLiveIndividual(me.data?.id, win, compare, absent)

  const toggleHarness = (h: string) =>
    setSearch({ harness: search.harness === h ? undefined : h }, { replace: true })
  const widen =
    search.preset === '30d'
      ? undefined
      : () => setSearch({ preset: '30d', from: undefined, to: undefined })

  const header = (
    <HarnessHeader
      search={search}
      setSearch={setSearch}
      from={from}
      today={today}
      compare={compare}
      // The live fallback always shows the viewer's own usage, so its trail has no level label.
      crumbs={!absent && res ? [{ label: res.scope.label }] : []}
      notice={absent ? copy.serverMissing : null}
      refreshFailed={level.refreshFailed}
      onRetry={() => void usage.refetch()}
    />
  )

  if (me.isError)
    return <ViewerError header={header} fix="GET /api/users/me failed. Reload to retry." />
  if (!me.data || (usage.isPending && !absent)) return <LevelLoading header={header} />

  // ── Not found or not visible (coded 404) ──
  if (level.notVisible)
    return (
      <div className="flex flex-col gap-4">
        {header}
        <EmptyState icon={SquareTerminal} title={copy.notVisible}>
          {copy.notVisibleHint}
        </EmptyState>
      </div>
    )

  if (absent)
    return (
      <LiveFallbackLevel
        header={header}
        live={live}
        windowLabel={win.label}
        compare={compare}
        selected={search.harness}
        onToggle={toggleHarness}
        name={me.data.display_name}
      />
    )

  if (!res) return <UsageError header={header} onRetry={() => void usage.refetch()} />

  return (
    <div className={`flex flex-col gap-4 ${level.stale ? 'opacity-80' : ''}`}>
      {header}
      <UserLevel
        res={res}
        self={res.scope.user_id === me.data.id}
        viewerIsSuperuser={me.data.is_superuser}
        win={win}
        days={days}
        today={today}
        harnesses={level.harnesses}
        compare={compare}
        stale={level.stale}
        selected={search.harness}
        onToggle={toggleHarness}
        onWiden={widen}
      />
    </div>
  )
}
