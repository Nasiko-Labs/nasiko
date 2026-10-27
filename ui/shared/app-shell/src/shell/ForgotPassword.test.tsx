/** "Forgot password?" (ForgotPassword.tsx): only the desktop app running Nasiko on this computer shows it. */
import { screen, waitFor } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'
import { copy } from '@nasiko/app-core/shell/copy'
import { forgetResetProbe } from './localReset'

setupPinnedSeed()
// The answer is read once per page load; each test is a new load.
beforeEach(() => forgetResetProbe())

const STATE = '/__nasiko/local/reset/state'
const link = () => screen.queryByRole('link', { name: copy.forgotPassword.link })

/** Renders /login with the probe answered by `answer`, and returns once that answer has been taken in, so a missing
 *  link means the page decided against it (not that the probe hadn't answered yet). */
async function signInPageWith(answer: () => Response) {
  let answered = false
  server.use(
    http.get(STATE, () => {
      answered = true
      return answer()
    }),
  )
  renderApp('/login')
  await screen.findByRole('button', { name: copy.login.submit })
  await waitFor(() => expect(answered).toBe(true))
  await new Promise((resolve) => setTimeout(resolve, 50))
}

describe('forgot password', () => {
  it('opens the desktop app’s reset page when it runs Nasiko on this computer', async () => {
    server.use(http.get(STATE, () => HttpResponse.json({ desktop: true, resettable: true })))
    renderApp('/login')
    expect(await screen.findByRole('link', { name: copy.forgotPassword.link })).toHaveAttribute(
      'href',
      '/__nasiko/local/reset',
    )
    expect(screen.getByText(copy.forgotPassword.lead, { exact: false })).toBeInTheDocument()
  })

  it('shows nothing in a browser (the server’s 404)', async () => {
    await signInPageWith(() => new HttpResponse('Not Found', { status: 404 }))
    expect(link()).toBeNull()
  })

  it('shows nothing when the desktop app uses a hosted server', async () => {
    await signInPageWith(() => HttpResponse.json({ desktop: true, resettable: false }))
    expect(link()).toBeNull()
  })

  it('shows nothing for the SPA’s HTML', async () => {
    await signInPageWith(() => HttpResponse.text('<!doctype html><title>x</title>'))
    expect(link()).toBeNull()
  })

  it('shows nothing for a body that is not the desktop app’s', async () => {
    await signInPageWith(() => HttpResponse.json({ resettable: true }))
    expect(link()).toBeNull()
  })
})
