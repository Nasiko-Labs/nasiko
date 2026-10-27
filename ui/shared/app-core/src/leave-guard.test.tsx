import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Link,
  Outlet,
  RouterProvider,
} from '@tanstack/react-router'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { LeaveGuard } from './leave-guard'

function setup(dirty: boolean) {
  const root = createRootRoute({
    component: () => (
      <>
        {/* A local test route tree, not the app's registered one. */}
        <Link to={'/away' as never}>Away</Link>
        <Outlet />
      </>
    ),
  })
  const form = createRoute({
    getParentRoute: () => root,
    path: '/',
    component: () => (
      <>
        <p>Form page</p>
        <LeaveGuard when={dirty} />
      </>
    ),
  })
  const away = createRoute({
    getParentRoute: () => root,
    path: '/away',
    component: () => <p>Away page</p>,
  })
  const login = createRoute({
    getParentRoute: () => root,
    path: '/login',
    component: () => <p>Login page</p>,
  })
  const router = createRouter({
    routeTree: root.addChildren([form, away, login]),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  render(<RouterProvider router={router} />)
  return router
}

describe('LeaveGuard', () => {
  it('asks before leaving a dirty form: Stay keeps the page, Leave goes', async () => {
    const user = userEvent.setup()
    setup(true)
    await user.click(await screen.findByRole('link', { name: 'Away' }))
    expect(
      await screen.findByRole('alertdialog', { name: 'Leave without saving?' }),
    ).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Stay' }))
    expect(screen.getByText('Form page')).toBeInTheDocument()
    await user.click(screen.getByRole('link', { name: 'Away' }))
    await user.click(await screen.findByRole('button', { name: 'Leave' }))
    expect(await screen.findByText('Away page')).toBeInTheDocument()
  })

  it('lets a clean form go without asking', async () => {
    const user = userEvent.setup()
    setup(false)
    await user.click(await screen.findByRole('link', { name: 'Away' }))
    expect(await screen.findByText('Away page')).toBeInTheDocument()
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument()
  })

  it('never holds back the session-expiry redirect to /login', async () => {
    const router = setup(true)
    await screen.findByText('Form page')
    await router.navigate({ to: '/login' as never })
    expect(await screen.findByText('Login page')).toBeInTheDocument()
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument()
  })
})
