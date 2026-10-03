/**
 * Other tabs' sign-outs and sign-ins (review D2): broadcast + reload. What this tab does is a
 * document navigation (jsdom can't navigate, so each test hands the watcher a fake `page`).
 */
import { QueryClient } from '@tanstack/react-query'
import { createMemoryHistory } from '@tanstack/react-router'
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import { createAppRouter } from '@/app/router'
import { chatRegistry, clearChatRegistry } from '@/features/chat/registry'
import {
  bumpSignInGeneration,
  onSessionMessage,
  SESSION_CHANNEL,
  setSigningOut,
  type SessionMessage,
} from '@/lib/session'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { watchOtherTabs } from './sessionSync'
import { signOut } from './signOut'

setupPinnedSeed()

// Leaving stops chat turns first, which imports the chat chunk: warm it so timing is the watcher's.
beforeAll(async () => {
  await import('@/features/chat/registry')
})

const channels: BroadcastChannel[] = []
/** Another tab: its own channel object, so messages cross like between tabs. */
const otherTab = () => {
  const c = new BroadcastChannel(SESSION_CHANNEL)
  channels.push(c)
  return c
}
// Torn down even when a test fails, so a leaked watcher can't act on later tests' messages.
const stops: (() => void)[] = []
afterEach(() => {
  channels.splice(0).forEach((c) => c.close())
  stops.splice(0).forEach((s) => s())
  setSigningOut(false)
  clearChatRegistry()
})

/** This tab, at `url`, watching the others. */
function tab(url: string, queryClient = new QueryClient()) {
  const router = createAppRouter({
    queryClient,
    history: createMemoryHistory({ initialEntries: [url] }),
  })
  const invalidate = vi.spyOn(router, 'invalidate').mockResolvedValue()
  const page = { assign: vi.fn(), replace: vi.fn(), reload: vi.fn() }
  stops.push(watchOtherTabs({ queryClient, router: router as never, page }))
  return { page, invalidate, queryClient }
}

const riley = { sub: 'u1', username: 'riley', is_superuser: false }

describe('another tab signs out', () => {
  it('stops chat turns, then loads /login with the failure notice and a way back', async () => {
    const { page, queryClient } = tab('/tokenops?range=7d')
    const clearAll = vi.spyOn(chatRegistry(queryClient, 'u1'), 'clearAll')
    otherTab().postMessage({ type: 'signed-out', failed: true } satisfies SessionMessage)
    await vi.waitFor(() =>
      expect(page.assign).toHaveBeenCalledWith(
        '/login?redirect=%2Ftokenops%3Frange%3D7d&signout=failed',
      ),
    )
    expect(clearAll).toHaveBeenCalled()
  })

  it('a confirmed sign-out loads /login without the notice', async () => {
    const { page } = tab('/sessions')
    otherTab().postMessage({ type: 'signed-out', failed: false } satisfies SessionMessage)
    await vi.waitFor(() => expect(page.assign).toHaveBeenCalledWith('/login?redirect=%2Fsessions'))
  })

  it('a tab already on /login reloads in place, gaining the notice', async () => {
    const { page } = tab('/login?expired=true')
    otherTab().postMessage({ type: 'signed-out', failed: true } satisfies SessionMessage)
    await vi.waitFor(() =>
      expect(page.replace).toHaveBeenCalledWith('/login?expired=true&signout=failed'),
    )
    expect(page.assign).not.toHaveBeenCalled()
  })

  it("another tab's successful Try again drops this /login's stale notice", async () => {
    const { page } = tab('/login?signout=failed&redirect=%2Fagents')
    otherTab().postMessage({ type: 'signed-out', failed: false, generation: '0' })
    await vi.waitFor(() => expect(page.replace).toHaveBeenCalledWith('/login?redirect=%2Fagents'))
  })

  it('drops a sign-out older than this browser’s latest sign-in', async () => {
    const { page } = tab('/agents')
    bumpSignInGeneration()
    const c = otherTab()
    c.postMessage({ type: 'signed-out', failed: false, generation: 'an-older-generation' })
    // One sender's messages arrive in order: once this sentinel is handled, the stale one was too.
    c.postMessage({ type: 'signed-in' } satisfies SessionMessage)
    await vi.waitFor(() => expect(page.reload).toHaveBeenCalled())
    expect(page.assign).not.toHaveBeenCalled()
  })

  it('stays put when this tab signs in while the chat chunk loads', async () => {
    const { page } = tab('/login')
    // Listeners run in order: this one is this tab's sign-in landing right after the watcher saw the message.
    stops.push(onSessionMessage(() => bumpSignInGeneration()))
    const c = otherTab()
    c.postMessage({ type: 'signed-out', failed: false } satisfies SessionMessage)
    c.postMessage({ type: 'signed-in' } satisfies SessionMessage) // sentinel
    await vi.waitFor(() => expect(page.assign).toHaveBeenCalledWith('/'))
    expect(page.replace).not.toHaveBeenCalled()
  })
})

describe('another tab signs in', () => {
  it('as an unknown account: stops chat turns, then reloads', async () => {
    const { page, queryClient } = tab('/agents')
    queryClient.setQueryData(['me'], riley)
    const clearAll = vi.spyOn(chatRegistry(queryClient, 'u1'), 'clearAll')
    otherTab().postMessage({ type: 'signed-in' } satisfies SessionMessage)
    await vi.waitFor(() => expect(page.reload).toHaveBeenCalled())
    expect(clearAll).toHaveBeenCalled()
  })

  it.each([
    ['a different account', { type: 'signed-in', sub: 'u2' }],
    ['a malformed sub (unknown account)', { type: 'signed-in', sub: 42 }],
    ['an empty sub (unknown account)', { type: 'signed-in', sub: '' }],
  ])('as %s: reloads', async (_name, message) => {
    const { page, queryClient } = tab('/agents')
    queryClient.setQueryData(['me'], riley)
    otherTab().postMessage(message)
    await vi.waitFor(() => expect(page.reload).toHaveBeenCalled())
  })

  it('reloads when this tab has no loaded account to compare with', async () => {
    const { page } = tab('/agents')
    otherTab().postMessage({ type: 'signed-in', sub: 'u1' } satisfies SessionMessage)
    await vi.waitFor(() => expect(page.reload).toHaveBeenCalled())
  })

  it('as the same account: keeps pages, data, chat turns and mutations; only the guard re-checks', async () => {
    const queryClient = new QueryClient()
    queryClient.setQueryData(['me'], riley)
    queryClient.setQueryData(['kept'], 1)
    const turns = vi.spyOn(chatRegistry(queryClient, 'u1'), 'clearAll')
    // A mutation in flight is this account's own work: it must still land.
    let finish!: () => void
    const onSuccess = vi.fn()
    const mutation = queryClient
      .getMutationCache()
      .build(queryClient, { mutationFn: () => new Promise<void>((r) => (finish = r)), onSuccess })
    const done = mutation.execute(undefined)
    const { page, invalidate } = tab('/agents', queryClient)
    otherTab().postMessage({ type: 'signed-in', sub: 'u1' } satisfies SessionMessage)
    await vi.waitFor(() => expect(invalidate).toHaveBeenCalled())
    expect(page.reload).not.toHaveBeenCalled()
    expect(queryClient.getQueryData(['kept'])).toBe(1)
    expect(turns).not.toHaveBeenCalled()
    finish()
    await done
    expect(onSuccess).toHaveBeenCalled()
  })

  it('a tab on /login continues to its redirect (or the default page)', async () => {
    const a = tab('/login?signout=failed&redirect=%2Fagents')
    const b = tab('/login?expired=true')
    otherTab().postMessage({ type: 'signed-in', sub: 'u1' } satisfies SessionMessage)
    await vi.waitFor(() => expect(a.page.assign).toHaveBeenCalledWith('/agents'))
    await vi.waitFor(() => expect(b.page.assign).toHaveBeenCalledWith('/'))
  })

  it('is ignored while this tab signs out (its own flow decides)', async () => {
    const { page, invalidate } = tab('/agents')
    setSigningOut(true)
    const c = otherTab()
    c.postMessage({ type: 'signed-in', sub: 'u2' } satisfies SessionMessage)
    // A sign-out is handled even mid sign-out: once this sentinel lands, the sign-in was seen and ignored.
    c.postMessage({ type: 'signed-out', failed: false } satisfies SessionMessage)
    await vi.waitFor(() => expect(page.assign).toHaveBeenCalled())
    expect(page.reload).not.toHaveBeenCalled()
    expect(invalidate).not.toHaveBeenCalled()
  })
})

describe('the watcher', () => {
  it('ignores malformed messages', async () => {
    const { page, invalidate } = tab('/agents')
    const c = otherTab()
    c.postMessage({ type: 'signed-out' })
    c.postMessage('nonsense')
    await new Promise((r) => setTimeout(r, 50))
    expect(page.assign).not.toHaveBeenCalled()
    expect(page.reload).not.toHaveBeenCalled()
    expect(invalidate).not.toHaveBeenCalled()
  })

  it('stops listening once unsubscribed', async () => {
    const { page } = tab('/sessions')
    stops.pop()!()
    // A second, live watcher is the sentinel: once it has handled the message, the first had its chance.
    const live = tab('/sessions')
    otherTab().postMessage({ type: 'signed-in' } satisfies SessionMessage)
    await vi.waitFor(() => expect(live.page.reload).toHaveBeenCalled())
    expect(page.reload).not.toHaveBeenCalled()
  })
})

describe('this tab signs out', () => {
  it('tells the other tabs', async () => {
    const got: SessionMessage[] = []
    otherTab().onmessage = (e: MessageEvent<SessionMessage>) => got.push(e.data)
    await signOut({ queryClient: new QueryClient(), userId: 'u1', navigate: async () => {} })
    await vi.waitFor(() =>
      expect(got).toEqual([{ type: 'signed-out', failed: false, generation: expect.any(String) }]),
    )
  })
})
