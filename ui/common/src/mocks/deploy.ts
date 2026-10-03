/**
 * Builds and uploads mock (plans/feat-deploy.md §9), derived from the agents mock so every agent's versions have their
 * builds. Shapes mirror nasiko-cloud-rs `2d6178e4`: `build/routes.rs` (`BuildRecord`, `Paginated` whose `total` is the page
 * length), `agents/upload.rs` (`UploadStatusItem` via `row_to_status_item`, `deploy_status_sse`).
 *
 * Each build has a timeline of (time, build status, upload status). In the browser the clock moves, so the live builds
 * advance on their own; tests pin the clock and move a build with `deployMockState().advance()`.
 */
import type { AgentsState } from './agents'
import type {
  BuildRecord,
  BuildStatus,
  UploadPipelineStatus,
  UploadStatus,
} from '@/features/deploy/types'

export interface MockBuild {
  record: Omit<BuildRecord, 'status' | 'updated_at'>
  ownerId: string
  agentName: string
  /** Oldest first; the entry in force is the last one at or before now. */
  timeline: { at: number; build: BuildStatus | null; upload: UploadPipelineStatus | null }[]
  /** An upload_status row exists (zip or GitHub clone); registry imports and CLI builds have none. */
  hasUpload: boolean
  errorMessage: string | null
}

export interface DeployState {
  builds: MockBuild[]
  /** Ids the SSE answers `not_found` for while the upload row survives (a failed first upload deleted its agent, D-4). */
  orphans: Set<string>
  /** Uploads whose agent row changes when they settle (`settleUploads`). */
  pending: { buildId: string; agentId: string; firstUpload: boolean }[]
  /** The viewer's GitHub connection; null means "as the `?mock=` variant says". */
  github: { connected: boolean | null; login: string }
}

const pad = (n: number) => String(n).padStart(12, '0')
export const buildId = (n: number) => `5eed000b-0000-4000-8000-${pad(n)}`
const iso = (ms: number) => new Date(ms).toISOString()

/** The live demo builds and the failure cases, by agent index in the seed (agents.ts `statusFor`). */
export const DEMO = { building: 1, queued: 4, failed: 5, conflict: 3 } as const
export const ORPHAN_ID = buildId(999)

export function buildDeployState(agents: AgentsState, now: number): DeployState {
  const builds: MockBuild[] = []
  const list = agents.agents.filter((a) => !a.harness)
  list.forEach((a, i) => {
    // Seed GitHub builds model rows made by POST /api/builds (the only route that writes github_url, D-12).
    const github = i % 3 === 2
    const key = a.name.replace(/^seed-/, '')
    for (const v of a.versions) {
      const at = Date.parse(v.created_at)
      builds.push({
        record: {
          id: v.build_id ?? buildId(i * 10),
          agent_id: a.id,
          github_url: github ? `https://github.com/acme/${key}` : null,
          commit_hash: github
            ? `c0ffee${pad(i).slice(-6)}${v.version.replace(/\./g, '')}`.slice(0, 40)
            : null,
          version_tag: v.version,
          image_reference: v.image_tag,
          logs_url: null,
          created_at: iso(at - 150_000),
        },
        ownerId: a.owner_id,
        agentName: a.name,
        timeline: [
          { at: at - 150_000, build: 'queued', upload: 'initiated' },
          { at: at - 140_000, build: 'building', upload: 'processing' },
          { at: at - 20_000, build: 'success', upload: 'orchestration_processing' },
          { at, build: 'success', upload: 'completed' },
        ],
        hasUpload: true,
        errorMessage: null,
      })
    }
  })
  const agentAt = (i: number) => list[i]
  const next = (v: string) => v.replace(/(\d+)$/, (d) => String(Number(d) + 1))
  const add = (
    i: number,
    n: number,
    timeline: MockBuild['timeline'],
    errorMessage: string | null = null,
    github = false,
    version?: string,
  ) => {
    const a = agentAt(i)
    if (!a) return
    const v = version ?? next(a.version)
    builds.push({
      record: {
        id: buildId(n),
        agent_id: a.id,
        github_url: github ? `https://github.com/acme/${a.name.replace(/^seed-/, '')}` : null,
        commit_hash: github ? 'deadbeef00000000000000000000000000000001' : null,
        version_tag: v,
        image_reference: `registry.local/${a.name.replace(/^seed-/, '')}:${v}`,
        logs_url: null,
        created_at: iso(timeline[0]!.at),
      },
      ownerId: a.owner_id,
      agentName: a.name,
      timeline,
      hasUpload: true,
      errorMessage,
    })
  }
  // A running build (advances in the browser: success about a minute after the page loads).
  add(DEMO.building, 1, [
    { at: now - 45_000, build: 'queued', upload: 'initiated' },
    { at: now - 40_000, build: 'building', upload: 'processing' },
    { at: now + 60_000, build: 'success', upload: 'orchestration_processing' },
    { at: now + 70_000, build: 'success', upload: 'completed' },
  ])
  // A queued one.
  add(DEMO.queued, 2, [
    { at: now - 3_000, build: 'queued', upload: 'initiated' },
    { at: now + 15_000, build: 'building', upload: 'processing' },
    { at: now + 90_000, build: 'success', upload: 'completed' },
  ])
  // A failed zip (the seed's failed agent).
  add(
    DEMO.failed,
    3,
    [
      { at: now - 3 * 3_600_000, build: 'queued', upload: 'initiated' },
      { at: now - 3 * 3_600_000 + 5_000, build: 'building', upload: 'processing' },
      { at: now - 3 * 3_600_000 + 9_000, build: 'failed', upload: 'failed' },
    ],
    'no Dockerfile found in root of zip',
  )
  // A GitHub clone whose version already existed (github.rs VERSION_CONFLICT).
  const c = agentAt(DEMO.conflict)
  if (c) {
    add(
      DEMO.conflict,
      4,
      [
        { at: now - 26 * 3_600_000, build: 'queued', upload: 'initiated' },
        { at: now - 26 * 3_600_000 + 4_000, build: 'failed', upload: 'failed' },
      ],
      `VERSION_CONFLICT:${c.version}:${next(c.version)}:${c.name} version ${c.version} already exists and versions are immutable`,
      true,
      c.version,
    )
  }
  // A failed first upload: the agent row (and its builds) were deleted; only the upload row survives.
  builds.push({
    record: {
      id: ORPHAN_ID,
      agent_id: '5eed000b-0000-4000-8000-00000000dead',
      github_url: null,
      commit_hash: null,
      version_tag: '0.1.0',
      image_reference: 'registry.local/new-bot:0.1.0',
      logs_url: null,
      created_at: iso(now - 2 * 3_600_000),
    },
    ownerId: agents.agents[0]?.owner_id ?? '',
    agentName: 'new-bot',
    timeline: [
      { at: now - 2 * 3_600_000, build: null, upload: 'initiated' },
      { at: now - 2 * 3_600_000 + 6_000, build: null, upload: 'failed' },
    ],
    hasUpload: true,
    errorMessage:
      'no Python entrypoint found (main.py, src/main.py, __main__.py, or src/__main__.py)',
  })
  return {
    builds,
    orphans: new Set([ORPHAN_ID]),
    pending: [],
    github: { connected: null, login: 'octocat' },
  }
}

function phase(b: MockBuild, now: number) {
  let cur = b.timeline[0]!
  for (const t of b.timeline) if (t.at <= now) cur = t
  return cur
}

export function buildStatusAt(b: MockBuild, now: number): BuildStatus | null {
  return phase(b, now).build
}

/** `build/routes.rs` `BuildRecord` as JSON; null for an orphan (its row was cascade-deleted). */
export function recordAt(b: MockBuild, now: number): BuildRecord | null {
  const p = phase(b, now)
  if (!p.build) return null
  return { ...b.record, status: p.build, updated_at: iso(Math.min(p.at, now)) }
}

const PROGRESS: Record<string, number> = {
  initiated: 10,
  processing: 30,
  capabilities_generated: 50,
  orchestration_triggered: 60,
  orchestration_processing: 80,
  completed: 100,
}
function message(status: string, name: string): string {
  switch (status) {
    case 'initiated':
      return `Upload initiated for '${name}'`
    case 'processing':
      return 'Processing agent source...'
    case 'capabilities_generated':
      return 'Agent capabilities generated'
    case 'orchestration_triggered':
      return 'Deployment triggered'
    case 'orchestration_processing':
      return 'Agent is being deployed...'
    case 'completed':
      return `Agent '${name}' deployed successfully`
    case 'failed':
      return 'Deployment failed'
    default:
      return status
  }
}

/** `upload.rs` `row_to_status_item`. */
export function uploadAt(b: MockBuild, now: number): UploadStatus | null {
  if (!b.hasUpload) return null
  const p = phase(b, now)
  const status = p.upload ?? 'initiated'
  const created = b.timeline[0]!.at
  const updated = Math.min(p.at, now)
  const done = status === 'completed' || status === 'failed'
  const progress = PROGRESS[status] ?? 0
  return {
    upload_id: b.record.id,
    agent_name: b.agentName,
    status,
    progress_percentage: progress,
    source_info: { filename: `${b.agentName}.zip`, content_type: 'application/zip' },
    file_size: 0,
    capabilities_generated: [
      'capabilities_generated',
      'orchestration_triggered',
      'orchestration_processing',
      'completed',
    ].includes(status),
    orchestration_triggered: [
      'orchestration_triggered',
      'orchestration_processing',
      'completed',
    ].includes(status),
    registry_updated: done && progress === 100,
    agent_url: null,
    registry_id: null,
    status_message: message(status, b.agentName),
    error_details: status === 'failed' && b.errorMessage ? [b.errorMessage] : [],
    validation_errors: [],
    created_at: iso(created),
    updated_at: iso(updated),
    completed_at: done ? iso(updated) : null,
    processing_duration: (updated - created) / 1000,
    orchestration_duration: null,
  }
}

/** Move a build to a new phase now (tests; the browser's clock moves the demo builds). */
export function advanceBuild(
  b: MockBuild,
  now: number,
  build: BuildStatus | null,
  upload: UploadPipelineStatus | null,
) {
  b.timeline = [...b.timeline.filter((t) => t.at <= now), { at: now, build, upload }]
}

// ── Uploads (POST /api/agents/upload) ──────────────────────────────────────────────────────

let seq = 100
/** A new upload's timeline: the browser's clock moves it (queued → building → deployed in about 25 s); tests pin it. */
export function uploadTimeline(now: number, fails: string | null): MockBuild['timeline'] {
  return fails
    ? [
        { at: now, build: 'queued', upload: 'initiated' },
        { at: now + 2_000, build: 'building', upload: 'processing' },
        { at: now + 8_000, build: 'failed', upload: 'failed' },
      ]
    : [
        { at: now, build: 'queued', upload: 'initiated' },
        { at: now + 2_000, build: 'building', upload: 'processing' },
        { at: now + 20_000, build: 'success', upload: 'orchestration_processing' },
        { at: now + 25_000, build: 'success', upload: 'completed' },
      ]
}

/**
 * Queue an upload like `upload_and_deploy`: a new agent row reads `deploying` until the build settles (then `running`,
 * or deleted with its builds if it was the agent's first upload and it failed, D-4); an existing agent keeps its status.
 */
export function queueUpload(
  state: DeployState,
  agents: AgentsState,
  opts: {
    name: string
    version: string
    ownerId: string
    now: number
    fails: string | null
    githubUrl?: string
    commit?: string
  },
) {
  const existing = agents.agents.find((a) => a.name === opts.name && !a.deleted)
  const n = seq++
  const agentId = existing?.id ?? `5eed000c-0000-4000-8000-${pad(n)}`
  if (!existing) {
    agents.agents.push({
      id: agentId,
      name: opts.name,
      display_name: opts.name,
      description: '',
      owner_id: opts.ownerId,
      status: 'deploying',
      version: opts.version,
      tags: [],
      skills: [],
      capabilities: {},
      created_at: iso(opts.now),
      updated_at: iso(opts.now),
      image: null,
      icon_url: null,
      documentation_url: null,
      is_public: false,
      harness: null,
      deleted: false,
      versions: [],
      deployment: null,
      secrets: [],
      userGrants: [],
      agentAcl: [],
      rollback: null,
    })
  }
  const id = buildId(n)
  state.builds.push({
    record: {
      id,
      agent_id: agentId,
      github_url: opts.githubUrl ?? null,
      commit_hash: opts.commit ?? null,
      version_tag: opts.version,
      image_reference: `registry.local/${opts.name}:${opts.version}`,
      logs_url: null,
      created_at: iso(opts.now),
    },
    ownerId: opts.ownerId,
    agentName: opts.name,
    timeline: uploadTimeline(opts.now, opts.fails),
    hasUpload: true,
    errorMessage: opts.fails,
  })
  state.pending.push({ buildId: id, agentId, firstUpload: !existing })
  return { buildId: id, agentId }
}

/** Apply finished uploads to the agents mock (called before agent reads): live on success, first uploads deleted on failure. */
export function settleUploads(state: DeployState, agents: AgentsState, now: number) {
  state.pending = state.pending.filter((p) => {
    const b = state.builds.find((x) => x.record.id === p.buildId)
    const up = b ? uploadAt(b, now) : null
    const agent = agents.agents.find((a) => a.id === p.agentId)
    if (!b || !up || !agent) return false
    if (up.status === 'completed') {
      agent.status = 'running'
      agent.version = b.record.version_tag
      agent.image = b.record.image_reference
      agent.versions.forEach((v) => {
        v.is_active = false
        v.status = 'archived'
      })
      agent.versions.push({
        id: `5eed000d-0000-4000-8000-${b.record.id.slice(-12)}`,
        agent_id: agent.id,
        build_id: b.record.id,
        version: b.record.version_tag,
        image_tag: b.record.image_reference,
        changelog: null,
        is_active: true,
        can_rollback: false,
        previous_version: null,
        status: 'active',
        created_at: iso(now),
      })
      return false
    }
    if (up.status === 'failed') {
      if (p.firstUpload) {
        // `delete_agent_or_mark_failed`: the agent row goes, and agent_builds cascade-delete with it.
        agents.agents.splice(agents.agents.indexOf(agent), 1)
        b.timeline = b.timeline.map((t) => ({ ...t, build: null }))
        state.orphans.add(b.record.id)
      } else agent.status = 'failed'
      return false
    }
    return true
  })
}

/**
 * A small multipart/form-data reader for the upload mock: Node's `Request.formData()` rejects bodies sent by jsdom's
 * XHR (an undici assertion), so the mock parses the bytes itself, the same way in the browser and in tests.
 */
export async function readMultipart(request: Request): Promise<Map<string, string | Blob>> {
  const type = request.headers.get('content-type') ?? ''
  const boundary = /boundary=([^;]+)/.exec(type)?.[1]?.replace(/^"|"$/g, '')
  const out = new Map<string, string | Blob>()
  if (!boundary) return out
  const body = new Uint8Array(await request.arrayBuffer())
  const dec = new TextDecoder()
  const marker = new TextEncoder().encode(`--${boundary}`)
  const find = (from: number) => {
    outer: for (let i = from; i <= body.length - marker.length; i++) {
      for (let j = 0; j < marker.length; j++) if (body[i + j] !== marker[j]) continue outer
      return i
    }
    return -1
  }
  let at = find(0)
  while (at >= 0) {
    const start = at + marker.length
    if (body[start] === 45 && body[start + 1] === 45) break // `--` ends the body
    const next = find(start)
    if (next < 0) break
    const part = body.subarray(start + 2, next - 2) // skip the CRLF after the marker and before the next one
    let headerEnd = -1
    for (let i = 0; i < part.length - 3; i++) {
      if (part[i] === 13 && part[i + 1] === 10 && part[i + 2] === 13 && part[i + 3] === 10) {
        headerEnd = i
        break
      }
    }
    if (headerEnd >= 0) {
      const headers = dec.decode(part.subarray(0, headerEnd))
      const name = /name="([^"]*)"/.exec(headers)?.[1]
      const filename = /filename="([^"]*)"/.exec(headers)?.[1]
      const content = part.subarray(headerEnd + 4)
      if (name)
        out.set(name, filename !== undefined ? new Blob([content.slice()]) : dec.decode(content))
    }
    at = next
  }
  return out
}

// ── GitHub (github.rs) ───────────────────────────────────────────────────────────────────────

/** `GitHubRepo` rows: one per seed agent (so a clone can clash with an existing version), plus two new ones. */
export function mockRepos(agents: AgentsState, now: number) {
  const day = 86_400_000
  const fromAgents = agents.agents
    .filter((a) => !a.harness && !a.deleted)
    .slice(0, 12)
    .map((a, i) => {
      // The repo is named after the agent, so a clone targets that agent (and can clash with its version).
      const key = a.name
      return {
        id: 1000 + i,
        name: key,
        full_name: `acme/${key}`,
        description: a.description || null,
        private: i % 4 === 1,
        clone_url: `https://github.com/acme/${key}.git`,
        ssh_url: `git@github.com:acme/${key}.git`,
        html_url: `https://github.com/acme/${key}`,
        default_branch: i % 5 === 3 ? 'develop' : 'main',
        updated_at: iso(now - (i + 2) * day),
        agentName: a.name,
      }
    })
  return [
    {
      id: 2001,
      name: 'fresh-agent',
      full_name: 'acme/fresh-agent',
      description: 'A brand-new agent',
      private: false,
      clone_url: 'https://github.com/acme/fresh-agent.git',
      ssh_url: 'git@github.com:acme/fresh-agent.git',
      html_url: 'https://github.com/acme/fresh-agent',
      default_branch: 'main',
      updated_at: iso(now - 3_600_000),
      agentName: 'fresh-agent',
    },
    {
      id: 2002,
      name: 'website',
      full_name: 'acme/website',
      description: 'Marketing site (not an agent)',
      private: true,
      clone_url: 'https://github.com/acme/website.git',
      ssh_url: 'git@github.com:acme/website.git',
      html_url: 'https://github.com/acme/website',
      default_branch: 'main',
      updated_at: iso(now - day),
      agentName: 'website',
    },
    ...fromAgents,
  ]
}

// ── Registry import (catalog/import.rs import_registry) ───────────────────────────────────────

/**
 * A finished synchronous import: the agent row (new, or the existing one updated) and, for an OpenRuntime agent package
 * (anything under `<host>/nasiko/`), a successful build; a plain image is pulled and has no build. `running: false` is the
 * server's 201 with `container_name: null` (the deploy failed, D-7).
 */
export function completeImport(
  state: DeployState,
  agents: AgentsState,
  opts: {
    name: string
    version: string
    image: string
    ownerId: string
    now: number
    built: boolean
    running: boolean
  },
) {
  const n = seq++
  let agent = agents.agents.find((a) => a.name === opts.name && !a.deleted)
  if (!agent) {
    agent = {
      id: `5eed000c-0000-4000-8000-${pad(n)}`,
      name: opts.name,
      display_name: opts.name,
      description: '',
      owner_id: opts.ownerId,
      status: 'deploying',
      version: opts.version,
      tags: [],
      skills: [],
      capabilities: {},
      created_at: iso(opts.now),
      updated_at: iso(opts.now),
      image: null,
      icon_url: null,
      documentation_url: null,
      is_public: false,
      harness: null,
      deleted: false,
      versions: [],
      deployment: null,
      secrets: [],
      userGrants: [],
      agentAcl: [],
      rollback: null,
    }
    agents.agents.push(agent)
  }
  const id = opts.built ? buildId(n) : null
  if (id) {
    state.builds.push({
      record: {
        id,
        agent_id: agent.id,
        github_url: null,
        commit_hash: null,
        version_tag: opts.version,
        image_reference: opts.image,
        logs_url: null,
        created_at: iso(opts.now - 90_000),
      },
      ownerId: opts.ownerId,
      agentName: opts.name,
      timeline: [
        { at: opts.now - 90_000, build: 'queued', upload: null },
        { at: opts.now - 85_000, build: 'building', upload: null },
        { at: opts.now, build: 'success', upload: null },
      ],
      hasUpload: false,
      errorMessage: null,
    })
  }
  agent.status = opts.running ? 'running' : 'failed'
  agent.version = opts.version
  agent.image = opts.image
  agent.versions.forEach((v) => {
    v.is_active = false
    v.status = 'archived'
  })
  agent.versions.push({
    id: `5eed000d-0000-4000-8000-${pad(n)}`,
    agent_id: agent.id,
    build_id: id,
    version: opts.version,
    image_tag: opts.image,
    changelog: null,
    is_active: true,
    can_rollback: false,
    previous_version: null,
    status: 'active',
    created_at: iso(opts.now),
  })
  return { agentId: agent.id, buildId: id }
}
