/**
 * The routed chat mock behaves like nasiko-server at cb3aaf0c (v1b §5.13): the assistant row is
 * written server-side at Done only when the stream ran to the end, flows record the calls, and a
 * resolved routed request's continuation saves exactly one reply however the client reconnects.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { readSse } from '@/lib/sse'
import type { ChatScenario } from './chat'
import { chatMockMessages, chatMockReconnects, configureChatMock, resetChatMock } from './chatStore'
import { configureMocks } from './handlers'
import { RATE_LIMIT_BODY, TRUNCATION_MARKER } from '@/features/chat/serverContract'

const url = (p: string) => new URL(p, globalThis.location.origin)
const post = (p: string, body: unknown) =>
  fetch(url(p), {
    method: 'POST',
    body: JSON.stringify(body),
    headers: { 'Content-Type': 'application/json' },
  })
let n = 0
const newSid = () => `5eedc000-0000-4000-8000-${String(++n).padStart(12, '0')}`

async function readAll(res: Response): Promise<unknown[]> {
  const out: unknown[] = []
  await readSse(res, (evs) => evs.forEach((e) => out.push(JSON.parse(e.data))))
  return out
}

async function routedTurn(scenario: ChatScenario, text = 'deploy it') {
  configureChatMock({ scenario })
  const sid = newSid()
  expect((await post('/api/chat/sessions', { session_id: sid, first_prompt: text })).status).toBe(
    201,
  )
  await post(`/api/chat/sessions/${sid}/messages`, { role: 'user', content: text })
  const res = await post('/api/orchestrator/a2a', {
    jsonrpc: '2.0',
    id: 1,
    method: 'message/stream',
    params: {
      message: { messageId: 'm', role: 'ROLE_USER', parts: [{ text }], contextId: sid },
      metadata: { session_id: sid },
    },
  })
  return { sid, res }
}

const assistants = (sid: string) => chatMockMessages(sid).filter((m) => m.role === 'assistant')

beforeEach(() => configureMocks({ loggedIn: true }))
afterEach(() => resetChatMock())

describe('routed stream', () => {
  it('saves one assistant row at Done with the turn trace id and flows the calls', async () => {
    const { sid, res } = await routedTurn('routed-multi-agent')
    const frames = await readAll(res)
    const trace = JSON.stringify(frames).match(/"trace_id":"(5eedf\w+)"/)![1]!
    const [row] = assistants(sid)
    expect(assistants(sid)).toHaveLength(1)
    expect(row).toMatchObject({
      trace_id: trace,
      content: 'Deploy 42 failed on a missing secret. Add it to the staging vault and redeploy.',
      metadata: null,
    })
    const flow = (await (await fetch(url(`/api/flows/${trace}`))).json()) as {
      steps: { depth: number; caller_agent_name: string; agent_name: string }[]
    }
    expect(flow.steps.map((s) => [s.depth, s.caller_agent_name])).toEqual([
      [1, 'orchestrator'],
      [1, 'orchestrator'],
    ])
  })

  it('saves nothing for an empty reply or a dropped stream', async () => {
    const empty = await routedTurn('routed-empty')
    await readAll(empty.res)
    expect(assistants(empty.sid)).toEqual([])
    const cut = await routedTurn('routed-cut')
    await expect(readAll(cut.res)).rejects.toThrow()
    expect(assistants(cut.sid)).toEqual([])
  })

  it('pre-stream errors match the server: JSON-RPC bodies, a plain-text 429', async () => {
    for (const [sc, code] of [
      ['routed-400', 400],
      ['routed-500', 500],
      ['routed-503', 503],
    ] as const) {
      const { res } = await routedTurn(sc)
      expect(res.status).toBe(code)
      expect(await res.json()).toMatchObject({
        jsonrpc: '2.0',
        id: null,
        error: { code: expect.any(Number) },
      })
    }
    const limited = await routedTurn('routed-429')
    expect(limited.res.status).toBe(429)
    expect(limited.res.headers.get('Content-Type')).toMatch(/text\/plain/)
  })
})

describe('routed resume lifecycle (EN-10)', () => {
  async function paused(scenario: ChatScenario) {
    const { sid, res } = await routedTurn(scenario)
    await readAll(res)
    const pending = (await (await fetch(url('/api/hitl/pending'))).json()) as {
      data: { id: string; execution: { origin: string; chat_session_id: string } }[]
    }
    const req = pending.data.find((r) => r.execution.chat_session_id === sid)!
    expect(req.execution.origin).toBe('orchestrator')
    expect(assistants(sid)).toEqual([])
    const resolved = await post(`/api/hitl/${req.id}/resolve`, { answer: 'us-east-1' })
    expect(resolved.status).toBe(200)
    return { sid, id: req.id }
  }
  const reconnect = (id: string) =>
    post('/api/orchestrator/a2a', {
      jsonrpc: '2.0',
      id: 2,
      method: 'message/stream',
      params: {
        message: { messageId: 'r', role: 'ROLE_USER', parts: [], contextId: 'x' },
        metadata: { reconnect_after_hitl_id: id },
      },
    })

  it('no reconnect: the continuation still saved one reply', async () => {
    const { sid } = await paused('routed-hitl-no-reconnect')
    expect(assistants(sid)).toHaveLength(1)
  })

  it('a reconnect replays the buffer and saves nothing more; repeated reconnects too', async () => {
    const { sid, id } = await paused('routed-hitl-repeat')
    const first = await readAll(await reconnect(id))
    await readAll(await reconnect(id))
    expect(JSON.stringify(first)).toContain('Done: deployed to us-east-1.')
    expect(chatMockReconnects()).toEqual([id, id])
    expect(assistants(sid)).toHaveLength(1)
  })

  it('a cancelled reconnect still leaves one reply', async () => {
    const cancelled = await paused('routed-hitl-cancelled')
    // MSW's Node interceptor never settles an awaited body cancel; the reply was saved at resolve anyway.
    void (await reconnect(cancelled.id)).body?.cancel().catch(() => undefined)
    expect(assistants(cancelled.sid)).toHaveLength(1)
  })

  it('an expired buffer is a 200 that stays open and silent, and the reply was saved anyway', async () => {
    const expired = await paused('routed-hitl-expired')
    const res = await reconnect(expired.id)
    expect(res.status).toBe(200)
    const reader = res.body!.getReader()
    const first = await Promise.race([
      reader.read().then(() => 'frame'),
      new Promise((r) => setTimeout(() => r('silent'), 50)),
    ])
    expect(first).toBe('silent')
    void reader.cancel().catch(() => undefined)
    expect(assistants(expired.sid)).toHaveLength(1)
  })

  it("the replay starts with the sub-agent's own stream; the saved reply is the orchestrator's only", async () => {
    const { sid, id } = await paused('routed-hitl-repeat')
    const frames = JSON.stringify(await readAll(await reconnect(id)))
    expect(frames.indexOf('Sub-agent: region us-east-1 confirmed.')).toBeLessThan(
      frames.indexOf('trace_meta'),
    )
    expect(assistants(sid)[0]!.content).toBe('Done: deployed to us-east-1.')
  })

  it("the truncation marker is the server's exact bytes, and 429 its exact text", async () => {
    const { id } = await paused('routed-hitl-truncated')
    const res = await reconnect(id)
    const raw = await res.text()
    expect(raw).toContain(`data: ${TRUNCATION_MARKER}`)
    const limited = await routedTurn('routed-429')
    expect(await limited.res.text()).toBe(RATE_LIMIT_BODY)
  })

  it('a full buffer ends with the truncation marker', async () => {
    const { sid, id } = await paused('routed-hitl-truncated')
    expect(JSON.stringify(await readAll(await reconnect(id)))).toContain('[replay truncated:')
    expect(assistants(sid)).toHaveLength(1)
  })

  it('reconnect refusals: 400 and 403 as JSON-RPC errors', async () => {
    const a = await paused('routed-reconnect-400')
    expect((await reconnect(a.id)).status).toBe(400)
    const b = await paused('routed-reconnect-403')
    expect((await reconnect(b.id)).status).toBe(403)
  })
})
