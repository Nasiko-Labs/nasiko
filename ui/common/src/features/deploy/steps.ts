/**
 * The Build page's four stages from three sources, newest wins (plans/feat-deploy.md §5): the build status (stream or
 * record), the upload pipeline status, and the agent's status once the image is built. Pure; tested in logic.test.ts.
 *
 * The server reports no per-stage timestamps (D-4): stage times exist only for transitions this page saw live.
 */
import type { VariantProps } from 'class-variance-authority'
import type { badgeVariants } from '@/components/ui/badge'
import { fmtDuration } from '@/lib/format'
import { copy } from './copy'
import type { BuildStatus, UploadStatus } from './types'

export const STAGES = ['queued', 'building', 'deploying', 'running'] as const
export type StageId = (typeof STAGES)[number]
export type StageState = 'done' | 'current' | 'pending' | 'failed' | 'warning'
export type Outcome = 'running' | 'failed' | 'notRunning'

export interface Stage {
  id: StageId
  label: string
  state: StageState
}

export interface BuildView {
  stages: Stage[]
  current: StageId | null
  outcome: Outcome | null
  /** Nothing more will change: the stream can close and the clock stops. */
  terminal: boolean
  badge: BadgeKey
}

export interface BuildInputs {
  /** The latest stream frame's status or the build record's; `not_found` after a failed first upload (D-4). */
  status: BuildStatus | 'not_found' | null
  upload?: Pick<UploadStatus, 'status'> | null
  /** The agent's own status, read once the image is built. */
  agentStatus?: string | null
}

/** Upload pipeline steps that mean the image is built and the deploy has started (`upload.rs` `status_progress`). */
const DEPLOYING = new Set(['orchestration_triggered', 'orchestration_processing'])
const AGENT_DOWN = new Set(['failed', 'crashed', 'stopped'])

/** The build status to trust: a missing or `not_found` stream answer falls back to the upload row (D-4 race). */
function effectiveStatus({ status, upload }: BuildInputs): BuildStatus | null {
  // Newest wins: a finished upload row outranks a build status that still says in progress (a stale record or a
  // dropped stream); the upload only finishes after the build did.
  const inProgress =
    !status || status === 'not_found' || status === 'queued' || status === 'building'
  if (inProgress && upload?.status === 'failed') return 'failed'
  if (inProgress && upload?.status === 'completed') return 'success'
  if (status && status !== 'not_found') return status
  switch (upload?.status) {
    case 'failed':
      return 'failed'
    case 'completed':
      return 'success'
    case 'initiated':
      return 'queued'
    case 'processing':
    case 'capabilities_generated':
      return 'building'
    default:
      return upload && DEPLOYING.has(upload.status) ? 'success' : null
  }
}

function stagesAt(index: number, last: StageState): Stage[] {
  return STAGES.map((id, i) => ({
    id,
    label: copy.steps[id],
    state: i < index ? 'done' : i === index ? last : 'pending',
  }))
}

export function deriveBuild(inputs: BuildInputs): BuildView {
  const status = effectiveStatus(inputs)
  const deploying = !!inputs.upload && DEPLOYING.has(inputs.upload.status)
  const agent = inputs.agentStatus ?? null
  switch (status) {
    case 'queued':
      return {
        stages: stagesAt(0, 'current'),
        current: 'queued',
        outcome: null,
        terminal: false,
        badge: 'queued',
      }
    case 'building':
      return deploying
        ? {
            stages: stagesAt(2, 'current'),
            current: 'deploying',
            outcome: null,
            terminal: false,
            badge: 'building',
          }
        : {
            stages: stagesAt(1, 'current'),
            current: 'building',
            outcome: null,
            terminal: false,
            badge: 'building',
          }
    case 'success': {
      // A known-down agent wins over the upload row's `completed` (written at deploy time, before any later crash).
      if (agent && AGENT_DOWN.has(agent)) {
        return {
          stages: stagesAt(3, 'warning'),
          current: null,
          outcome: 'notRunning',
          terminal: true,
          badge: 'notRunning',
        }
      }
      if (agent === 'running' || inputs.upload?.status === 'completed') {
        return {
          stages: stagesAt(4, 'done'),
          current: null,
          outcome: 'running',
          terminal: true,
          badge: 'running',
        }
      }
      return {
        stages: stagesAt(2, 'current'),
        current: 'deploying',
        outcome: null,
        terminal: false,
        badge: 'building',
      }
    }
    case 'failed':
      // The image build (or the clone) failed, unless the upload had already reached the deploy.
      return {
        stages: stagesAt(deploying ? 2 : 1, 'failed'),
        current: null,
        outcome: 'failed',
        terminal: true,
        badge: 'failed',
      }
    default:
      return {
        stages: stagesAt(0, 'pending'),
        current: null,
        outcome: null,
        terminal: false,
        badge: 'queued',
      }
  }
}

/** One badge vocabulary for the Builds list, the agent Builds tab and the Build page (design review 12). */
type BadgeVariant = NonNullable<VariantProps<typeof badgeVariants>['variant']>
export type BadgeKey = 'queued' | 'building' | 'running' | 'success' | 'failed' | 'notRunning'
export const BUILD_BADGE: Record<BadgeKey, { variant: BadgeVariant; label: string }> = {
  queued: { variant: 'muted', label: copy.status.queued },
  building: { variant: 'info', label: copy.status.building },
  running: { variant: 'success', label: copy.status.running },
  success: { variant: 'success', label: copy.status.success },
  failed: { variant: 'destructive', label: copy.status.failed },
  notRunning: { variant: 'warning', label: copy.status.notRunning },
}

export const isActive = (status: BuildStatus): boolean =>
  status === 'queued' || status === 'building'

/** `owner/name` from a GitHub URL (`https://github.com/acme/bot.git` → `acme/bot`). */
export const repoOf = (url: string) =>
  url
    .replace(/^https?:\/\/github\.com\//, '')
    .replace(/\.git$/, '')
    .replace(/\/$/, '')

/** Where a build came from, from what the record carries. */
export function buildSource(b: { github_url: string | null; commit_hash: string | null }): string {
  if (b.github_url) return copy.source.github(repoOf(b.github_url), b.commit_hash)
  return copy.source.upload
}

/** Whole seconds for a build clock (no ticking decimals, design review 13): `48 s`, `1 m 12 s`, `1 h 05 m`. */
export function fmtElapsed(ms: number): string {
  // Whole seconds under a minute (fmtDuration would show milliseconds there); fmtDuration's `m ss s` / `h mm m` above.
  return ms < 60_000 ? `${Math.max(0, Math.floor(ms / 1000))} s` : fmtDuration(ms)
}
