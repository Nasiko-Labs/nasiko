// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { buildAgentsState } from './agents'
import {
  buildDeployState,
  queueUpload,
  recordAt,
  settleUploads,
  uploadAt,
  uploadTimeline,
} from './deploy'
import { generateHarnessSeed } from './seed-harness'
import { generateSeed } from './seed'

const NOW = Date.parse('2026-03-20T15:00:00Z')
const fresh = () => {
  const agents = buildAgentsState(
    generateSeed({ anchor: new Date(NOW) }),
    generateHarnessSeed(),
    NOW,
  )
  return { agents, deploy: buildDeployState(agents, NOW) }
}

describe('deploy mock (plans/feat-deploy.md §9)', () => {
  it('gives every agent version a successful build and keeps the demo cases', () => {
    const { agents, deploy } = fresh()
    const versions = agents.agents.flatMap((a) => a.versions.map((v) => v.build_id))
    for (const id of versions)
      expect(
        recordAt(
          deploy.builds.find((b) => b.record.id === id)!,
          NOW,
        )?.status,
      ).toBe('success')
    const statuses = deploy.builds.map((b) => recordAt(b, NOW)?.status ?? null)
    expect(statuses).toEqual(expect.arrayContaining(['queued', 'building', 'failed', null]))
  })

  it('puts a new agent live when its first upload completes', () => {
    const { agents, deploy } = fresh()
    const { buildId, agentId } = queueUpload(deploy, agents, {
      name: 'fresh-bot',
      version: '0.1.0',
      ownerId: 'u',
      now: NOW,
      fails: null,
    })
    expect(agents.agents.find((a) => a.id === agentId)?.status).toBe('deploying')
    const later = NOW + 30_000
    expect(
      uploadAt(
        deploy.builds.find((b) => b.record.id === buildId)!,
        later,
      )?.status,
    ).toBe('completed')
    settleUploads(deploy, agents, later)
    expect(agents.agents.find((a) => a.id === agentId)).toMatchObject({
      status: 'running',
      version: '0.1.0',
    })
    expect(deploy.pending).toEqual([])
  })

  it('deletes a new agent and its build when the first upload fails (D-4), keeping the upload row', () => {
    const { agents, deploy } = fresh()
    const { buildId, agentId } = queueUpload(deploy, agents, {
      name: 'broken-bot',
      version: '0.1.0',
      ownerId: 'u',
      now: NOW,
      fails: 'upload and deploy failed',
    })
    settleUploads(deploy, agents, NOW + 10_000)
    expect(agents.agents.some((a) => a.id === agentId)).toBe(false)
    const b = deploy.builds.find((x) => x.record.id === buildId)!
    expect(recordAt(b, NOW + 10_000)).toBeNull()
    expect(uploadAt(b, NOW + 10_000)).toMatchObject({
      status: 'failed',
      error_details: ['upload and deploy failed'],
    })
  })

  it('keeps an existing agent (marked failed) when a later upload fails', () => {
    const { agents, deploy } = fresh()
    const a = agents.agents.find((x) => x.versions.length && !x.harness)!
    queueUpload(deploy, agents, {
      name: a.name,
      version: '9.9.9',
      ownerId: 'u',
      now: NOW,
      fails: 'upload and deploy failed',
    })
    settleUploads(deploy, agents, NOW + 10_000)
    expect(agents.agents.find((x) => x.id === a.id)?.status).toBe('failed')
  })

  it('moves a new upload through queued → building → success → completed', () => {
    const t = uploadTimeline(NOW, null).map((x) => [x.build, x.upload])
    expect(t).toEqual([
      ['queued', 'initiated'],
      ['building', 'processing'],
      ['success', 'orchestration_processing'],
      ['success', 'completed'],
    ])
  })
})
