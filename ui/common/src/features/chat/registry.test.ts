/**
 * The app wiring in registry.ts: the real replyExists, the beforeunload guard, a user switch.
 */
import { QueryClient } from '@tanstack/react-query'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { server } from '@/test/setup'
import { clearDrafts } from './drafts'
import type { TurnRegistry } from './turnRegistry'

type Deps = Parameters<typeof import('./turnRegistry').createTurnRegistry>[0]
const captured: { deps: Deps[]; fakes: { busy: boolean; clearAll: ReturnType<typeof vi.fn> }[] } = {
  deps: [],
  fakes: [],
}

vi.mock('./turnRegistry', async (orig) => {
  const real = await orig<typeof import('./turnRegistry')>()
  return {
    ...real,
    createTurnRegistry: (deps: Deps) => {
      captured.deps.push(deps)
      const fake = { busy: false, clearAll: vi.fn() }
      captured.fakes.push(fake)
      // The signals (v1c §5.8) subscribe to the registry when it's built.
      return {
        busy: () => fake.busy,
        clearAll: fake.clearAll,
        subscribe: () => () => undefined,
        subscribeEnds: () => () => undefined,
        snapshot: () => [],
      } as unknown as TurnRegistry
    },
  }
})

const { chatRegistry, clearChatRegistry } = await import('./registry')

afterEach(() => {
  clearChatRegistry()
  captured.deps.length = 0
  captured.fakes.length = 0
  clearDrafts()
  vi.restoreAllMocks()
})

const leave = () => {
  const e = new Event('beforeunload', { cancelable: true })
  window.dispatchEvent(e)
  return e.defaultPrevented
}

describe('registry wiring', () => {
  it('replyExists finds an identical reply after the user row, and gives up when the row is not on the newest page', async () => {
    chatRegistry(new QueryClient(), 'u1')
    const { replyExists } = captured.deps[0]!
    const row = (id: string, role: string, content: string) => ({
      id,
      session_id: 's',
      role,
      content,
      timestamp: '2026-09-27T00:00:00Z',
    })
    server.use(
      http.get('/api/chat/sessions/s/messages', () =>
        HttpResponse.json({
          data: [
            row('a0', 'assistant', 'same'),
            row('u1', 'user', 'q'),
            row('a1', 'assistant', 'the reply'),
          ],
          has_more: false,
          hitl: [],
        }),
      ),
    )
    expect(await replyExists('s', 'u1', 'the reply')).toBe(true)
    expect(await replyExists('s', 'u1', 'other text')).toBe(false)
    // A matching reply before the user row does not count.
    expect(await replyExists('s', 'u1', 'same')).toBe(false)
    expect(await replyExists('s', 'not-on-page', 'the reply')).toBe(false)
  })

  it('warns before unload only while busy; a user switch clears the old registry and its listener', () => {
    const client = new QueryClient()
    const first = chatRegistry(client, 'u1')
    expect(chatRegistry(client, 'u1')).toBe(first)
    expect(leave()).toBe(false)
    captured.fakes[0]!.busy = true
    expect(leave()).toBe(true)

    const second = chatRegistry(client, 'u2')
    expect(second).not.toBe(first)
    expect(captured.fakes[0]!.clearAll).toHaveBeenCalledOnce()
    // The old registry's listener is gone: only the new one (idle) decides.
    expect(leave()).toBe(false)
    captured.fakes[1]!.busy = true
    expect(leave()).toBe(true)

    clearChatRegistry()
    expect(captured.fakes[1]!.clearAll).toHaveBeenCalledOnce()
    expect(leave()).toBe(false)
  })
})
