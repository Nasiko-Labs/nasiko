/**
 * Follows every deploy started from the setup guide's tabs (Registry, GitHub, Upload a zip) into the guide's deploy store
 * (api.ts), which the guide's progress rows, its footer summary and Home's setup and try-a-run cards read (NAS-1450).
 * The Starter pack deploys nothing (user request 2026-11-12), so the store follows what the user deploys themselves.
 *
 * Deploying: a registry import in flight, a zip uploading, or a build the follower still follows (a zip's or a GitHub
 * clone's). The outcome is the follower's (`onFollowFinished`), toasted or not: running, didn't start, or failed. One
 * watch per signed-in user for the page's life, so Home keeps showing a deploy after the guide closes.
 */
import { ApiError } from '@nasiko/data-access/client'
import {
  followedBuilds,
  onFollowFinished,
  subscribeFollower,
} from '@nasiko/features/deploy/follower'
import { importFor, subscribeUploads, uploadFor } from '@nasiko/features/deploy/uploads'
import { seededDeployOf, setSeededDeploy } from './api'
import { copy } from './copy'

/** An agent's name from a registry reference (`host[:port]/owner/name[:tag]`): its last path segment, untagged. */
export const nameOfReference = (reference: string) =>
  (reference.split('/').pop() ?? reference).split(':')[0] ?? reference

/** The server's plain-text reason when it sent one, as the Registry tab shows it. */
const reasonOf = (e: unknown) =>
  e instanceof ApiError && typeof e.body === 'string' && e.body
    ? e.body
    : e instanceof Error
      ? e.message
      : String(e)

/** `name` is deploying, unless it already is (its clock keeps its start). */
function deploying(name: string, startedAt: number) {
  if (seededDeployOf(name)?.phase !== 'deploying')
    setSeededDeploy(name, { phase: 'deploying', title: name, startedAt })
}

function failed(name: string, reason: string) {
  const d = seededDeployOf(name)
  if (d?.phase === 'failed' && d.reason === reason) return
  setSeededDeploy(name, { phase: 'failed', title: name, reason })
}

/** Reads what is in flight for `userId` now and writes it to the store. */
function sync(userId: string) {
  const imported = importFor(userId)
  if (imported?.phase === 'importing')
    deploying(nameOfReference(imported.reference), imported.startedAt)
  if (imported?.phase === 'failed')
    failed(nameOfReference(imported.reference), reasonOf(imported.error))
  const uploaded = uploadFor(userId)
  if (uploaded?.phase === 'uploading') deploying(uploaded.fields.name, Date.now())
  if (uploaded?.phase === 'failed') failed(uploaded.fields.name, reasonOf(uploaded.error))
  for (const b of followedBuilds()) deploying(b.name, b.since)
}

let watching: { userId: string; stop: () => void } | null = null

/** Starts following `userId`'s deploys (idempotent); a different user replaces the watch. */
export function watchGuideDeploys(userId: string) {
  if (watching?.userId === userId) return
  watching?.stop()
  const offUploads = subscribeUploads(() => sync(userId))
  const offFollower = subscribeFollower(() => sync(userId))
  const offFinished = onFollowFinished((n) => {
    if (n.kind === 'failed') return failed(n.name, copy.agent.buildFailed)
    setSeededDeploy(n.name, {
      phase: n.kind === 'running' ? 'running' : 'notRunning',
      title: n.name,
      agentId: n.agentId ?? '',
    })
  })
  watching = {
    userId,
    stop: () => {
      offUploads()
      offFollower()
      offFinished()
    },
  }
  sync(userId)
}

/** Sign-out and tests: stop following. */
export function stopWatchingGuideDeploys() {
  watching?.stop()
  watching = null
}
