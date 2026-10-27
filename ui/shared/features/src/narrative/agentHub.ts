/**
 * The agent hub's sentence (plans/feat-agent-centric.md PR1 "One sentence"; design "Sentence templates"). At most two
 * templated sentences, each mapped to data:
 * - Money, from the ACL-scoped dashboard's current and previous-window totals for the selected window (DX: no
 *   timeseries read). Left out when the viewer can't see the agent's spend. No comparison when the previous window
 *   had no spend or only part of it existed for this agent; "since <created date>" when the agent is younger than
 *   the window.
 * - The latest recorded change, as its own sentence.
 *
 * Product Principle 1: a change in a number and a marker are separate statements. No sentence ties them with
 * "since", "because", "due to", "led to" or "after" (the copy test in features/hub/logic.test.ts bans them; "since" only ever
 * introduces the creation date in the money sentence).
 */
import type { Marker } from '@nasiko/features/hub/events'
import { fmtMoney, fmtShortDay, fmtUtcDayTime } from '@nasiko/ui/lib/format'
import type { Preset } from '@nasiko/ui/lib/search'

export interface HubNarrativeInput {
  preset: Preset
  /** The resolved window's label ("Last 30 days", "This month", "Sep 1 – Sep 9 (UTC)"). */
  windowLabel: string
  win: { start: Date; prevStart: Date }
  /** The dashboard's totals for this agent; null when the viewer can't see them (or they haven't loaded). */
  money: { current: number; previous: number; changePct: number | null; unpriced?: boolean } | null
  createdAt: string
  latest: Marker | null
}

const PERIODS: Partial<Record<Preset, { now: string; before: string }>> = {
  '24h': { now: 'in the last 24 hours', before: 'the 24 before' },
  '7d': { now: 'in the last 7 days', before: 'the 7 before' },
  '30d': { now: 'in the last 30 days', before: 'the 30 before' },
  mtd: { now: 'this month', before: 'the same span before it' },
}

function period(i: HubNarrativeInput) {
  return PERIODS[i.preset] ?? { now: `in ${i.windowLabel}`, before: 'the period before' }
}

function moneySentence(i: HubNarrativeInput): string | null {
  if (!i.money) return null
  const created = Date.parse(i.createdAt)
  const p = period(i)
  // Unpriced calls make the total a floor: "at least", "no priced spend", and no comparison (review: Codex).
  const floor = !!i.money.unpriced
  const amount = `${floor ? 'at least ' : ''}${fmtMoney(i.money.current)}`
  const none = floor ? 'No priced spend' : 'No spend'
  if (!Number.isNaN(created) && created >= i.win.start.getTime()) {
    const since = fmtShortDay(new Date(created))
    return i.money.current > 0
      ? `Spent ${amount} since ${since}, when it was created.`
      : `${none} since ${since}, when it was created.`
  }
  if (i.money.current <= 0) return `${none} ${p.now}.`
  const spent = `Spent ${amount} ${p.now}`
  // Only part of the previous window existed for this agent, or it had no spend: no comparison clause.
  const partial = !Number.isNaN(created) && created > i.win.prevStart.getTime()
  const pct = i.money.changePct
  if (floor || partial || i.money.previous <= 0 || pct === null) return `${spent}.`
  const rounded = Math.round(Math.abs(pct))
  if (rounded === 0) return `${spent}, about the same as ${p.before}.`
  return `${spent}, ${rounded}% ${pct < 0 ? 'less' : 'more'} than ${p.before}.`
}

function when(m: Marker, hourly: boolean) {
  // A 24 h window spans two UTC days: name the day and the zone with the hour.
  return hourly ? `on ${fmtUtcDayTime(new Date(m.at))} UTC` : `on ${fmtShortDay(new Date(m.at))}`
}

function markerSentence(i: HubNarrativeInput): string | null {
  const m = i.latest
  if (!m) return null
  const at = when(m, i.preset === '24h')
  const v = m.version ? `v${m.version}` : null
  switch (m.kind) {
    case 'build':
      return m.sub === 'failed'
        ? `${v ? `A build of ${v}` : 'A build'} failed ${at}.`
        : `${v ? `Build ${v}` : 'A build'} completed ${at}.`
    case 'version':
      return m.version
        ? `Version ${m.version} was recorded ${at}.`
        : `A version was recorded ${at}.`
    case 'crash':
      return `It last crashed ${at}.`
    case 'event':
      // The server's summary leads with its own verb ("Token optimization turned on"): put the date first, so a summary
      // ending in "on" never reads "turned on on Sep 30" (QA ISSUE-001).
      return `O${at.slice(1)}: ${m.label.replace(/\.\s*$/, '')}.`
  }
}

/** The hub's sentences, in order: money, then the latest change. Empty when neither is known. */
export function hubNarrative(i: HubNarrativeInput): string[] {
  return [moneySentence(i), markerSentence(i)].filter((s): s is string => !!s)
}
