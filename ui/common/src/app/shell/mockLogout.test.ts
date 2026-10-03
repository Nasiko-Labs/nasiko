/**
 * The mocks' logged-out 401 carries the server's dead-session reason (signOut.ts treats it as signed out).
 */
import { describe, expect, it } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { setupPinnedSeed } from '@/test/pinnedSeed'

setupPinnedSeed()

describe('mock logged-out 401s', () => {
  const url = (p: string) => new URL(p, globalThis.location.origin).toString()

  it.each(['/api/me', '/api/observability/finops/dashboard'])(
    '%s answers with the dead-session reason "missing or invalid token"',
    async (path) => {
      configureMocks({ loggedIn: false })
      const res = await fetch(url(path))
      expect(res.status).toBe(401)
      expect(await res.json()).toEqual({
        data: null,
        status_code: 401,
        message: 'missing or invalid token',
      })
    },
  )
})
