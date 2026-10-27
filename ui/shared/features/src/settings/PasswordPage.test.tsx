import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { recordRequests, server } from '@nasiko/testing/setup'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy as shell } from '@nasiko/app-core/shell/copy'
import { copy as settings } from './copy'

const copy = settings.password

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 5000 }
const STRONG = 'A-brand-new-password9'

/** The page's form (the only one: the card holding the three fields and Change password). */
async function open() {
  const current = await screen.findByLabelText(copy.current, {}, T)
  return current.closest('form') as HTMLElement
}
async function fill(d: HTMLElement, current: string, next: string, confirm = next) {
  const set = async (label: string, v: string) => {
    const el = within(d).getByLabelText(label)
    await userEvent.clear(el)
    if (v) await userEvent.type(el, v)
  }
  await set(copy.current, current)
  await set(copy.next, next)
  await set(copy.confirm, confirm)
  await userEvent.click(within(d).getByRole('button', { name: copy.submit }))
}

describe('Change password (Settings → Account → Password)', () => {
  it("reports the policy's first broken rule on its field before sending", async () => {
    const rec = recordRequests()
    renderApp('/account/password')
    const d = await open()
    expect(within(d).getByText(copy.policy(12, 64))).toBeInTheDocument()
    await fill(d, 'whatever', 'short')
    expect(await within(d).findByText(copy.problem.short(12))).toBeInTheDocument()
    await fill(d, 'whatever', STRONG, 'different')
    expect(await within(d).findByText(copy.mismatch)).toBeInTheDocument()
    rec.stop()
    expect(rec.urls.some((u) => u.pathname === '/api/auth/change-password')).toBe(false)
  })

  it('each field has its own reveal toggle', async () => {
    renderApp('/account/password')
    const d = await open()
    const current = within(d).getByLabelText(copy.current)
    const toggle = within(d).getByRole('button', { name: copy.show(copy.current) })
    expect(current).toHaveAttribute('type', 'password')
    expect(toggle).toHaveAttribute('aria-pressed', 'false')
    await userEvent.click(toggle)
    expect(current).toHaveAttribute('type', 'text')
    expect(toggle).toHaveAttribute('aria-pressed', 'true')
    expect(within(d).getByLabelText(copy.next)).toHaveAttribute('type', 'password')
  })

  it('changes it, then a wrong current password lands on its field (403, never a sign-out)', async () => {
    const { router } = renderApp('/account/password')
    await fill(await open(), 'whatever', STRONG)
    expect(await screen.findByText(copy.changed, {}, T)).toBeInTheDocument()
    // The form remounts after a change: nothing typed before survives.
    await waitFor(() => expect(screen.getByLabelText(copy.current)).toHaveValue(''))
    const d = await open()
    expect(within(d).getByLabelText(copy.next)).toHaveValue('')
    await fill(d, 'not-it', 'Another-password9')
    expect(await within(d).findByText('Current password is incorrect')).toBeInTheDocument()
    expect(router.state.location.pathname).toBe('/account/password')
  })

  it('an SSO account gets the server reason as a toast (409 no_local_password)', async () => {
    server.use(
      http.post('/api/auth/change-password', () =>
        HttpResponse.json(
          {
            error: 'this account signs in through your identity provider',
            code: 'no_local_password',
          },
          { status: 409 },
        ),
      ),
    )
    renderApp('/account/password')
    await fill(await open(), 'whatever', STRONG)
    expect(
      await screen.findByText('This account signs in through your identity provider', {}, T),
    ).toBeInTheDocument()
  })

  it('a 204 (changed, no new session) signs out and says why on /login', async () => {
    server.use(
      http.post('/api/auth/change-password', () => {
        configureMocks({ loggedIn: false })
        return new HttpResponse(null, { status: 204 })
      }),
    )
    const { router } = renderApp('/account/password')
    await fill(await open(), 'whatever', STRONG)
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'), T)
    expect(router.state.location.search).toMatchObject({ password: 'changed' })
    expect(await screen.findByText(shell.login.passwordChanged)).toBeInTheDocument()
  })
})
