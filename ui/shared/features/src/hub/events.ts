/**
 * Change markers for the agent hub (plans/feat-agent-centric.md PR1 "Marker sources", eng "Markers"). Pure: the hub
 * page feeds it the reads, tests feed it records.
 *
 * Markers draw only from records the server keeps, with their own timestamp:
 * - Builds (`/api/builds/agent/{id}`, newest 20): a finished build is "Build completed" at its `updated_at` (labelled
 *   approximate: the server keeps no finish time of its own), a failed one "Build failed" at its failure time; queued
 *   and building records are not drawn. Nothing is called "deployed": a successful build is not a deployment (eng).
 * - Versions join their build through `AgentVersion.build_id`: one marker per deploy, labelled with the version. A
 *   version with no build (a registry import) is "Version recorded" at its own time; a version whose build is outside
 *   the read is not drawn as build-less. When the viewer can't read builds at all, every version is its own marker.
 * - The latest crash (`crashed_at` on the current deployment row): EE only, since the OSS server never writes it (O-1).
 * - The proposed event log (AH-3), mock mode only until a server has it.
 *
 * Ids (`?marker=`, X3): `build:<build id>`, `version:<version>`, `crash:<crashed_at ISO>`, `event:<event id>`.
 */
import type { AgentVersion } from '@nasiko/features/agents/types'
import type { BuildRecord } from '@nasiko/features/deploy/types'
import { copy } from './copy'
import { BUILDS_READ } from './tuning'
import type { AgentEvent } from './types'

type MarkerKind = 'build' | 'version' | 'crash' | 'event'

export interface Marker {
  id: string
  kind: MarkerKind
  /** The event log's kind (`restart`, `route_change`, …); the build's or crash's kind otherwise. */
  sub: string
  at: number
  /** What happened, short: "Build completed", "Crashed", or the event's own summary. */
  label: string
  /** The version, when the record names one (without the leading `v`). */
  version: string | null
  /** The time is the record's last update, not a recorded moment of the change itself. */
  approximate: boolean
  /** Feedback tone: a crash and a budget stop are drawn in status colours; everything else is neutral. */
  alert: boolean
  /** The build record behind the marker, for the "Open build" link. */
  buildId: string | null
  /** Who made the change, when the event log says. */
  actor: string | null
}

export interface MarkerInput {
  /** null when the viewer can't read builds (`{available:false}`) or the agent has none to read (a harness). */
  builds: readonly BuildRecord[] | null
  versions: readonly AgentVersion[] | null
  crashedAt: string | null
  events: readonly AgentEvent[] | null
  win: { start: Date; end: Date }
  /** Coding harnesses have no builds, versions or crashes: their markers come from the event log only (R2-10). */
  harness: boolean
}

export interface MarkerSet {
  /** In the window, oldest first. */
  markers: Marker[]
  /**
   * The builds read hit its cap while its oldest record is still inside the window, so older deploys in the window
   * may be missing: the axis says "latest 20 build records checked".
   */
  buildsCapped: boolean
}

const parse = (iso: string | null | undefined) => (iso ? Date.parse(iso) : Number.NaN)
const stripV = (v: string) => v.replace(/^v/i, '')

function buildMarkers(input: MarkerInput): Marker[] {
  const out: Marker[] = []
  const builds = input.builds ?? []
  const byBuild = new Map<string, AgentVersion>()
  for (const v of input.versions ?? []) if (v.build_id) byBuild.set(v.build_id, v)
  for (const b of builds) {
    if (b.status !== 'success' && b.status !== 'failed') continue
    const at = parse(b.updated_at)
    if (Number.isNaN(at)) continue
    const version = byBuild.get(b.id)?.version ?? b.version_tag
    out.push({
      id: `build:${b.id}`,
      kind: 'build',
      sub: b.status,
      at,
      label: b.status === 'success' ? copy.marker.buildDone : copy.marker.buildFailed,
      version: version ? stripV(version) : null,
      approximate: b.status === 'success',
      alert: false,
      buildId: b.id,
      actor: null,
    })
  }
  // A version string can be recorded twice (the registry overwrites a version in place): one marker per version,
  // the newest record, so `version:<v>` ids stay unique.
  const seen = new Set<string>()
  const newestFirst = [...(input.versions ?? [])].sort((a, b) =>
    b.created_at.localeCompare(a.created_at),
  )
  for (const v of newestFirst) {
    // Joined to a build in the read: that build's marker carries it. Built but outside the read: not drawn (eng).
    if (v.build_id && input.builds !== null) continue
    if (seen.has(v.version)) continue
    seen.add(v.version)
    const at = parse(v.created_at)
    if (Number.isNaN(at)) continue
    out.push({
      id: `version:${v.version}`,
      kind: 'version',
      sub: 'version',
      at,
      label: copy.marker.versionRecorded,
      version: stripV(v.version),
      approximate: false,
      alert: false,
      buildId: null,
      actor: null,
    })
  }
  return out
}

function eventMarkers(events: readonly AgentEvent[]): Marker[] {
  return events.flatMap((e) => {
    const at = parse(e.at)
    if (Number.isNaN(at)) return []
    return [
      {
        id: `event:${e.id}`,
        kind: 'event' as const,
        sub: e.kind,
        at,
        label: e.summary,
        version: null,
        approximate: false,
        alert: e.kind === 'crash' || e.kind === 'budget_stop',
        buildId: null,
        actor: e.actor,
      },
    ]
  })
}

export function markersFor(input: MarkerInput): MarkerSet {
  const from = input.win.start.getTime()
  const to = input.win.end.getTime()
  const all: Marker[] = []
  if (!input.harness) {
    all.push(...buildMarkers(input))
    const crashAt = parse(input.crashedAt)
    if (input.crashedAt && !Number.isNaN(crashAt))
      all.push({
        id: `crash:${input.crashedAt}`,
        kind: 'crash',
        sub: 'crash',
        at: crashAt,
        label: copy.marker.crashed,
        version: null,
        approximate: false,
        alert: true,
        buildId: null,
        actor: null,
      })
  }
  if (input.events) all.push(...eventMarkers(input.events))
  const inWindow = (m: Marker) => m.at >= from && m.at < to
  const builds = input.builds ?? []
  // The read's oldest record, by the time its marker would sit at; an unparseable time can't prove the cap was
  // outside the window, so it counts as inside (the coverage line shows rather than hides).
  const times = builds.map((b) => parse(b.updated_at))
  const oldest = times.some(Number.isNaN) ? from : Math.min(...times)
  return {
    markers: all.filter(inWindow).sort((a, b) => a.at - b.at || a.id.localeCompare(b.id)),
    buildsCapped: builds.length >= BUILDS_READ && oldest >= from,
  }
}

/** A marker's name everywhere it is listed: "Build completed · v1.4.2". */
export const markerText = (m: Marker) => (m.version ? `${m.label} · v${m.version}` : m.label)

/** The newest marker, for the hub's one sentence; null when the window has none. */
export const latestMarker = (set: MarkerSet): Marker | null => set.markers.at(-1) ?? null

const ID_RE = /^(build|version|crash|event):.+$/

/**
 * What to do with a `?marker=` id (X3): keep it while the markers read is pending or failed (never judged on a
 * failed read), `found` when it is drawn, `drop` when the reads succeeded and it isn't (outside the window, or no such
 * record in what was read), and `drop` at once for an id that isn't `<kind>:<record id>`.
 */
export function markerIdState(
  id: string | undefined,
  set: MarkerSet | null,
): 'none' | 'pending' | 'found' | 'drop' {
  if (!id) return 'none'
  if (!ID_RE.test(id)) return 'drop'
  if (!set) return 'pending'
  return set.markers.some((m) => m.id === id) ? 'found' : 'drop'
}
