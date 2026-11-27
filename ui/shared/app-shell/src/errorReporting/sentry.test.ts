import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it, vi } from 'vitest'
import { meQuery } from '@nasiko/data-access/auth'

const setUser = vi.fn()
vi.mock('@sentry/react', () => ({ setUser: (user: unknown) => setUser(user) }))
vi.mock('@sentry/replay/worker-bundler?url', () => ({ default: '' }))

const { followSignedInUser } = await import('./sentry')

describe('followSignedInUser', () => {
  it('names the signed-in user by id only, follows sign-in and sign-out, and does nothing without a client', () => {
    followSignedInUser(undefined)
    expect(setUser).not.toHaveBeenCalled()

    const client = new QueryClient()
    followSignedInUser(client)
    expect(setUser).toHaveBeenLastCalledWith(null)

    client.setQueryData(meQuery.queryKey, { sub: 'u-123', username: 'ada', is_superuser: false })
    expect(setUser).toHaveBeenLastCalledWith({ id: 'u-123' })

    client.removeQueries({ queryKey: meQuery.queryKey })
    expect(setUser).toHaveBeenLastCalledWith(null)
  })
})
