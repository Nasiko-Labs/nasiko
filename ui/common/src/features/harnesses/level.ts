/**
 * What every Harnesses page shares, whichever edition renders it (docs/lab-vs-react-migration-review.md §10.5): the
 * window, one level's usage with its "last good" fallback, and the usage endpoint's capability probe (a bare 404 means
 * absent, a coded 404 means not visible). The OSS page (HarnessesPage.tsx) and the EE layer's org page both build on it.
 */
import { useMemo, useState } from 'react'
import { compareOn } from '@/app/shell/context'
import { resolveWindow } from '@/features/tokenops/window'
import { env } from '@/lib/env'
import { useFrozenNow, useReturnTick } from '@/lib/useReturnTick'
import { isEndpointAbsent, isNotVisible, useUsage, type UsageRequest } from './api'
import { clampFrom, orderHarnesses, windowDays } from './rollup'
import type { HarnessesSearch } from './search'
import type { UsageResponse, UsageRow } from './types'
import type { PanelItem } from './components/HarnessPanels'

export type SetSearch<S = HarnessesSearch> = (
  patch: Partial<S>,
  opts?: { replace?: boolean },
) => void

/** Where the CLI connects: the configured server (in dev the Vite origin is not a nasiko server). */
export const serverOrigin = (() => {
  try {
    return env.legacyUiUrl ? new URL(env.legacyUiUrl).origin : location.origin
  } catch {
    return location.origin
  }
})()

/** Mocked usage (mock mode, or VITE_NASIKO_MOCK=harnesses) is a preview: seed ids the real Sessions page lacks. */
export const preview = env.mode === 'mock' || env.partialMocks.includes('harnesses')

/** The page's window: frozen "now", a custom `from` clamped to MAX_WINDOW_DAYS, and Compare. */
export function useHarnessWindow(
  search: Pick<HarnessesSearch, 'preset' | 'from' | 'to' | 'compare'>,
) {
  const returnTick = useReturnTick()
  const now = useFrozenNow(search.preset, search.from, search.to, returnTick)
  // A custom window is clamped to MAX_WINDOW_DAYS, measured back from its effective end (`to`, or
  // today when `to` is in the future): a hand-edited link must not ask the server for centuries.
  const today = now.toISOString().slice(0, 10)
  const from =
    search.preset === 'custom'
      ? clampFrom(search.from, search.to && search.to > today ? today : search.to)
      : search.from
  const win = useMemo(
    () => resolveWindow({ preset: search.preset, from, to: search.to }, now),
    [search.preset, from, search.to, now],
  )
  // Memoised so the Trend's own memos hold between renders.
  const days = useMemo(() => windowDays(win.start, win.end), [win])
  return { today, from, win, days, compare: compareOn(search) }
}

/**
 * One level's usage (`req`), paged by "Load more". `who` is the query identity (the mock persona, or the signed-in
 * user), so one account's cached usage never serves another's.
 */
export function useUsageLevel(
  who: string | undefined,
  req: UsageRequest,
  windowKey: string,
  enabled: boolean,
) {
  const usage = useUsage(who, req, windowKey, enabled)
  // A failed "Load more" keeps the loaded pages and turns the button into Retry; only a failed
  // first page (or refresh) is the level's error.
  const pageFailed = usage.isFetchNextPageError
  const levelError = pageFailed ? null : usage.error
  const absent = isEndpointAbsent(levelError)
  const notVisible = isNotVisible(levelError)

  // The last good pages for this level: when a refresh under a new window key fails, the
  // placeholder is gone (error state), so the page keeps these numbers and says so. Set during
  // render (React's "adjust state on change" pattern) so no ref is read while rendering.
  const reqKey = JSON.stringify([who, req])
  const [lastGood, setLastGood] = useState<{ key: string; data: typeof usage.data } | null>(null)
  if (
    usage.data &&
    !usage.isPlaceholderData &&
    (lastGood?.key !== reqKey || lastGood.data !== usage.data)
  )
    setLastGood({ key: reqKey, data: usage.data })
  const data =
    usage.data ??
    (usage.isError && !absent && !notVisible && lastGood?.key === reqKey
      ? lastGood.data
      : undefined)
  const res = data?.pages[0]
  const pages = data?.pages
  const rows: UsageRow[] = useMemo(() => pages?.flatMap((p) => p.rows) ?? [], [pages])
  const harnesses = useMemo(
    () => orderHarnesses((res?.by_harness ?? []).map((h) => h.harness)),
    [res],
  )
  return {
    usage,
    res,
    rows,
    lastPage: pages?.at(-1),
    harnesses,
    pageFailed,
    levelError,
    absent,
    notVisible,
    stale: usage.isPlaceholderData,
    // A failed refetch keeps the last good data: say so instead of passing it off as current.
    refreshFailed: !!levelError && !!res && !absent && !notVisible,
  }
}

/** The harness cards of a usage response, in display order. */
export function panelItems(res: Pick<UsageResponse, 'by_harness'>): PanelItem[] {
  const byId = new Map(res.by_harness.map((h) => [h.harness, h]))
  return orderHarnesses(byId.keys()).flatMap((h) => {
    const totals = byId.get(h)
    return totals ? [{ harness: h, totals, topModel: totals.top_models[0] }] : []
  })
}
