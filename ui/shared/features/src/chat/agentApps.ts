/**
 * The apps an agent can use in chat, from `GET /api/mcp/agents/{id}/my-connections`: the connectors enabled on the
 * agent, each with where the viewer stands. Tools run on the viewer's own connection, never the agent owner's, so
 * an app the viewer has not connected is one they can connect from here. Any agent with connectors enabled gets
 * this; nothing here knows which agents are the platform's own.
 */
import type { Skill } from '@nasiko/features/agents/types'
import type { CallerConnection } from '@nasiko/features/mcp/types'

export interface AgentApp {
  id: string
  name: string
  label: string
  logoUrl: string | null
  authType: string | null
  status: CallerConnection
}

/** How many apps the new-chat row shows before "+N more". */
export const MAX_SHOWN_APPS = 5

const fold = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, '')

/**
 * The words an agent's card uses for its apps, in the card's order: every skill tag, folded ("Google Drive" and
 * "googledrive" match). Each maps to where it first appears.
 */
function namedInCard(skills: readonly Skill[] | undefined): Map<string, number> {
  const out = new Map<string, number>()
  for (const s of skills ?? [])
    for (const t of s.tags ?? []) if (!out.has(fold(String(t)))) out.set(fold(String(t)), out.size)
  return out
}

/** Where the card first names `a`, or null when it doesn't. */
function cardPlace(a: AgentApp, named: Map<string, number>): number | null {
  const places = [named.get(fold(a.name)), named.get(fold(a.label))].filter((n) => n !== undefined)
  return places.length ? Math.min(...places) : null
}

/**
 * The apps worth showing, in order: the viewer's connected ones first (what the agent can use for them now), then
 * the ones still to connect. Within each, the apps the agent's card names come first, in the card's own order (an
 * agent with many connectors enabled still leads with the ones it is built around), then the rest by label. An app
 * the viewer cannot reach is left out: they could not connect it here.
 */
export function orderApps(apps: readonly AgentApp[], skills?: readonly Skill[]): AgentApp[] {
  const named = namedInCard(skills)
  const key = (a: AgentApp): [number, number, number] => {
    const place = cardPlace(a, named)
    return [a.status === 'needs_connection' ? 1 : 0, place === null ? 1 : 0, place ?? 0]
  }
  return apps
    .filter((a) => a.status !== 'not_shared')
    .slice()
    .sort((a, b) => {
      const [x, y] = [key(a), key(b)]
      return x[0] - y[0] || x[1] - y[1] || x[2] - y[2] || a.label.localeCompare(b.label)
    })
}

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/**
 * The apps a reply names that the viewer still has to connect (a "Try it on your own Gmail" tip, say). The name
 * must appear as the app writes it, so a weekday or a common word ("Monday", "linear") is not an app; and when the
 * agent's card names any of its apps, only those count.
 */
export function appsToConnectIn(
  text: string,
  apps: readonly AgentApp[],
  skills?: readonly Skill[],
): AgentApp[] {
  const named = namedInCard(skills)
  const cardNamesApps = apps.some((a) => cardPlace(a, named) !== null)
  return apps.filter(
    (a) =>
      a.status === 'needs_connection' &&
      (!cardNamesApps || cardPlace(a, named) !== null) &&
      new RegExp(`(^|[^\\w])${escape(a.label)}([^\\w]|$)`).test(text),
  )
}
