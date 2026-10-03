/**
 * The client half of the X6 smoke (v1b §5.14, R9): the routed stream captured from a live
 * nasiko-server by `node scripts/smoke-chat-routed.ts --record`, replayed through the real
 * registry. The client must save only the user row; the server saved the reply.
 * The replay fails loudly when the frames the UI depends on are missing (schema-drift canary).
 */
import { describe, expect, it, vi } from 'vitest'
import { sseResponse, type MockFrame } from '@/mocks/chat'
import fixture from './__fixtures__/live-routed.json'
import { agentActivity, agentsAsked } from './activity'
import { routedReplyText } from './a2aReducer'
import { isTruncationMarker } from './serverContract'
import { createTurnRegistry, isLivePhase } from './turnRegistry'
import type { ChatMessage, ChatSessionRow, SaveMessageBody } from './types'

const frames: MockFrame[] = (fixture.frames as unknown[]).map((data) => ({ data }))
const parts = (fixture.frames as unknown[])
  .flatMap(
    (f) =>
      (
        f as {
          statusUpdate?: { status?: { message?: { parts?: { data?: Record<string, unknown> }[] } } }
        }
      ).statusUpdate?.status?.message?.parts ?? [],
  )
  .flatMap((p) => (p.data ? [p.data] : []))

describe('live routed fixture', () => {
  it('still carries what the UI depends on: trace_meta, a named tool_call, a paired tool_result, artifact text', () => {
    expect(
      parts.find((p) => p.type === 'trace_meta')?.trace_id,
      'trace_meta.trace_id is missing: settle-by-trace (EN-2) has nothing to match',
    ).toBeTruthy()
    const call = parts.find((p) => p.type === 'tool_call')
    expect(
      typeof call?.agent,
      'tool_call.agent is missing: Activity and attribution have no names',
    ).toBe('string')
    expect(call?.agent).toBe(fixture.expectedAgent)
    const result = parts.find((p) => p.type === 'tool_result')
    expect(result?.turn, 'tool_result.turn is missing: FIFO pairing (G-1) breaks').toBe(call?.turn)
    // usage_meta, then the terminal COMPLETED (§2.4).
    const last = fixture.frames.at(-1) as { statusUpdate?: { status?: { state?: string } } }
    expect(last.statusUpdate?.status?.state).toBe('TASK_STATE_COMPLETED')
    expect(parts.at(-1)?.type).toBe('usage_meta')
    expect(
      (fixture.frames as unknown[]).some((f) => JSON.stringify(f).includes('artifactUpdate')),
    ).toBe(true)
    // A first send never carries the continuation's truncation marker.
    expect(parts.some((p) => typeof p.text === 'string' && isTruncationMarker(p.text))).toBe(false)
  })

  it('was scrubbed: no tokens, raw ids or agent text', () => {
    const text = JSON.stringify(fixture)
    expect(text).not.toContain('eyJ')
    expect(text).not.toMatch(/\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/i)
    expect(text).not.toMatch(/\b[0-9a-f]{32}\b/i)
    for (const p of parts)
      for (const k of ['result', 'content', 'message'])
        if (typeof p[k] === 'string' && p[k]) expect(p[k]).toMatch(/^<\w+ \d+ chars>$/)
  })

  it('replays through the real registry: only the user row is saved, the turn ends done with its agent', async () => {
    const saved: SaveMessageBody[] = []
    let n = 0
    const reg = createTurnRegistry({
      userId: 'u',
      createSession: vi.fn(
        async (b) =>
          ({
            session_id: b.session_id,
            agent_id: null,
            agent_url: null,
            title: 't',
            created_at: '',
          }) as ChatSessionRow,
      ),
      saveMessage: vi.fn(async (sessionId: string, body: SaveMessageBody) => {
        saved.push(body)
        return {
          id: `m-${saved.length}`,
          session_id: sessionId,
          role: body.role,
          content: body.content,
          timestamp: '',
        } as ChatMessage
      }),
      dispatch: vi.fn(async () => sseResponse(frames)),
      replyExists: vi.fn(async () => false),
      onUnauthorized: vi.fn(),
      newId: () => `id-${++n}`,
      now: () => 0,
    })
    const t = await reg.send({ chatMode: 'routed', text: fixture.prompt })
    await vi.waitFor(() => {
      const cur = reg.get(t.sessionId)
      if (!cur || isLivePhase(cur.phase)) throw new Error('still live')
    })
    const done = reg.get(t.sessionId)!
    expect(saved.map((s) => s.role)).toEqual(['user'])
    expect(done.phase).toBe('done')
    expect(done.state.traceId).toBe('trace-1')
    expect(agentsAsked(done.state)).toEqual([fixture.expectedAgent])
    expect(agentActivity(done.state.steps, true).map((a) => a.summary)).toEqual(['completed'])
    // The fixture keeps shape, not content: free-form text is a length placeholder.
    expect(routedReplyText(done.state)).toMatch(/^<text \d+ chars>/)
    // Sub-agent text is activity, kept per agent, never the reply.
    expect(done.state.agentNotes[fixture.expectedAgent]?.content).toBeTruthy()
  })
})
