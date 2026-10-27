/**
 * The ways into a flow (plans/feat-flows.md F19, F20; eng review T1, O2): "Open flow" on the trace page, a Sessions
 * row's detail, chat's Activity for a saved routed reply and a workflow run step that names its trace, each present
 * when its id is known and absent when it isn't; "Answer the request" on a paused flow, and the matching rule.
 * The five pages' own suites stay unchanged (the regression contract).
 */
import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import type { HitlDto } from '@nasiko/features/chat/types'
import { apiData } from '@nasiko/data-access/client'
import { configureMocks, flowsMockState, workflowsMockState } from '@nasiko/mocks/handlers'
import { SHOWCASE_SESSION } from '@nasiko/mocks/observability'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'
import { matchRequest } from './answer'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const flowHref = /^\/flows\/[0-9a-zA-Z-]+$/

describe('Open flow (F19)', () => {
  it('the trace page links the selected trace’s flow', async () => {
    renderApp(`/sessions/${SHOWCASE_SESSION}`)
    const link = await screen.findByRole('link', { name: 'Open flow' }, { timeout: 8000 })
    expect(link.getAttribute('href')).toMatch(flowHref)
  })

  it('a Sessions row’s detail links its latest flow', async () => {
    renderApp('/sessions?live=paused')
    const toggles = await screen.findAllByRole(
      'button',
      { name: /^Show details for/ },
      { timeout: 8000 },
    )
    await userEvent.click(toggles[0]!)
    const link = await screen.findByRole(
      'link',
      { name: /^Open (latest )?flow$/ },
      { timeout: 8000 },
    )
    expect(link.getAttribute('href')).toMatch(flowHref)
  })

  it('chat’s Activity for a saved routed reply links the turn’s flow', async () => {
    const CHAT = '5eedc000-0000-4000-8000-0000000fa0f1'
    const TRACE = '5eedf000000000000000000000000001'
    const at = (i: number) => new Date(Date.parse('2026-03-20T10:00:00Z') + i * 1000).toISOString()
    server.use(
      http.get('/api/chat/sessions', () =>
        HttpResponse.json({
          data: [
            {
              session_id: CHAT,
              agent_id: null,
              agent_url: null,
              title: 'A routed chat',
              created_at: at(0),
              updated_at: at(1),
              agent_name: null,
              is_coding_agent: false,
            },
          ],
          has_more: false,
          next_cursor: null,
          prev_cursor: null,
        }),
      ),
      http.get(`/api/chat/sessions/${CHAT}/messages`, () =>
        HttpResponse.json({
          data: [
            {
              id: 'u0',
              session_id: CHAT,
              role: 'user',
              content: 'q',
              timestamp: at(0),
              trace_id: null,
            },
            {
              id: 'a0',
              session_id: CHAT,
              role: 'assistant',
              content: 'answer',
              timestamp: at(1),
              trace_id: TRACE,
            },
          ],
          has_more: false,
          next_cursor: null,
          prev_cursor: 'u0',
          hitl: [],
        }),
      ),
      http.get(`/api/flows/${TRACE}`, () =>
        HttpResponse.json({
          flow: {},
          steps: [
            {
              step_order: 1,
              depth: 1,
              agent_name: 'seed-agent',
              caller_agent_name: 'orchestrator',
              status: 'completed',
            },
          ],
        }),
      ),
    )
    renderApp(`/chat/${CHAT}`)
    await userEvent.click(
      await screen.findByRole('button', { name: /^Activity/ }, { timeout: 8000 }),
    )
    expect(screen.getByRole('link', { name: 'Open flow' })).toHaveAttribute(
      'href',
      `/flows/${TRACE}`,
    )
  })

  it('a workflow run step links its flow only when the server named its trace', async () => {
    const run = workflowsMockState().runs.find((r) => r.maf_id)!
    const path = `/workflows/${run.maf_id}?run=${run.id}`
    // Today's mock steps carry no trace id: no link.
    renderApp(path)
    await screen.findAllByRole('heading', { level: 3 }, { timeout: 8000 })
    expect(screen.queryByRole('link', { name: 'Open flow' })).toBeNull()
  })

  it('…and with a trace id on a step, the link goes to that flow', async () => {
    const run = workflowsMockState().runs.find((r) => r.maf_id)!
    const body = await apiData<{ step_results: Record<string, unknown>[] }>(
      `/api/maf/execution/${run.id}`,
    )
    const TRACE = '5eedf000000000000000000000000002'
    const withTrace = {
      ...body,
      step_results: body.step_results.map((s, i) => (i === 0 ? { ...s, trace_id: TRACE } : s)),
    }
    server.use(
      http.get(`/api/maf/execution/${run.id}`, () =>
        HttpResponse.json({ data: withTrace, status_code: 200, message: 'ok' }),
      ),
    )
    renderApp(`/workflows/${run.maf_id}?run=${run.id}`)
    const links = await screen.findAllByRole('link', { name: 'Open flow' }, { timeout: 8000 })
    expect(links).toHaveLength(1)
    expect(links[0]).toHaveAttribute('href', `/flows/${TRACE}`)
  })
})

describe('Answer the request (F20, O2)', () => {
  it('a paused flow with no session sends you to chat’s Waiting view', async () => {
    const f = flowsMockState().flows.find((x) => x.title?.startsWith('Refund order'))!
    renderApp(`/flows/${f.id}`)
    const links = await screen.findAllByRole(
      'link',
      { name: copy.answer.waiting },
      { timeout: 8000 },
    )
    expect(links[0]).toHaveAttribute('href', '/chat')
  })

  it('matches exactly one pending request by chat and agent', () => {
    const req = (
      id: string,
      ctx: string | null,
      agent: string | null,
      status = 'pending',
    ): HitlDto =>
      ({
        id,
        status,
        execution: { context_id: ctx, chat_session_id: null, agent_id: agent },
      }) as unknown as HitlDto
    const rs = [
      req('a', 's1', 'g1'),
      req('b', 's1', 'g2'),
      req('c', 's2', 'g1'),
      req('d', 's1', 'g1', 'resolved'),
    ]
    expect(matchRequest(rs, 's1', 'g1')?.id).toBe('a')
    expect(matchRequest([...rs, req('e', 's1', 'g1')], 's1', 'g1')).toBeUndefined()
    expect(matchRequest(rs, null, 'g1')).toBeUndefined()
    expect(matchRequest(rs, 's1', null)).toBeUndefined()
    // A routed request names its chat in chat_session_id (its context is the sub-agent's).
    const routed = {
      id: 'r',
      status: 'pending',
      execution: { context_id: 'sub-ctx', chat_session_id: 's9', agent_id: 'g1' },
    } as unknown as HitlDto
    expect(matchRequest([routed], 's9', 'g1')?.id).toBe('r')
  })
})
