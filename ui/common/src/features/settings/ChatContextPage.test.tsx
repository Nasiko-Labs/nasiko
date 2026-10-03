import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { recordRequestBodies, server } from '@/test/setup'
import { renderApp } from '@/test/renderApp'
import { copy as settingsCopy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

// Radix Select needs pointer-capture APIs jsdom doesn't have (same stub as the TokenOps filter tests).
const proto = Element.prototype as unknown as Record<string, unknown>
const stubbed = [
  'hasPointerCapture',
  'releasePointerCapture',
  'setPointerCapture',
  'scrollIntoView',
].filter((k) => !(k in proto))
beforeAll(() => {
  for (const k of stubbed) proto[k] = k === 'hasPointerCapture' ? () => false : () => {}
})
afterAll(() => {
  for (const k of stubbed) delete proto[k]
})

const T = { timeout: 5000 }
const copy = settingsCopy.chatContext

describe('Settings: Chat context (nasiko-cloud-rs context_selection.rs)', () => {
  it('a member sees the saved values and saves only the one they changed', async () => {
    configureMocks({ superuser: false })
    const bodies = recordRequestBodies()
    renderApp('/settings/chat-context')
    const strategy = await screen.findByRole('combobox', { name: copy.strategy }, T)
    expect(strategy).toHaveTextContent(copy.strategies.pacms)
    expect(screen.getByRole('combobox', { name: copy.budget })).toHaveTextContent(
      copy.levels.medium,
    )
    expect(screen.getByRole('button', { name: settingsCopy.save })).toBeDisabled()

    await userEvent.click(screen.getByRole('combobox', { name: copy.budget }))
    await userEvent.click(await screen.findByRole('option', { name: copy.levels.high }))
    await userEvent.click(screen.getByRole('button', { name: settingsCopy.save }))
    expect(await screen.findByText(copy.saved, {}, T)).toBeInTheDocument()
    await bodies.flush()
    expect(
      bodies.requests.filter((r) => r.method === 'PATCH').map((r) => [r.url.pathname, r.body]),
    ).toEqual([['/api/me/pacms-budget', { level: 'high' }]])
    // Re-read from the server after the save.
    expect(
      await within(document.body).findByRole('combobox', { name: copy.budget }, T),
    ).toHaveTextContent(copy.levels.high)
  })

  it('says a server without the routes needs a newer one', async () => {
    server.use(http.get('/api/me/pacms-budget', () => new HttpResponse(null, { status: 404 })))
    renderApp('/settings/chat-context')
    expect(await screen.findByText(copy.newerServer, {}, T)).toBeInTheDocument()
  })
})
