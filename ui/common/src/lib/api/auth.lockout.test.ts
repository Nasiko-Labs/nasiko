/** Login lockout (auth/service.rs, ea233d20): `lockedFor` reads the 429 body, `copy.login.locked` words the wait. */
import { describe, expect, it } from 'vitest'
import { copy } from '@/app/shell/copy'
import { lockedFor } from './auth'

describe('lockedFor', () => {
  it('is null for anything but account_locked (the plain rate limit); otherwise whole minutes, at least 1, else 15', () => {
    for (const body of [
      null,
      undefined,
      'Too Many Requests',
      {},
      { code: 'rate_limited', retry_after_secs: 60 },
    ])
      expect(lockedFor(body)).toBeNull()
    // Rounds the wait up to whole minutes, at least 1; without a usable wait, the server's 15-minute lock.
    expect(lockedFor({ code: 'account_locked', retry_after_secs: 840 })).toBe(14)
    expect(lockedFor({ code: 'account_locked', retry_after_secs: 841 })).toBe(15)
    expect(lockedFor({ code: 'account_locked', retry_after_secs: 30 })).toBe(1)
    for (const retry_after_secs of [undefined, 0, -5, 'soon', Number.POSITIVE_INFINITY])
      expect(lockedFor({ code: 'account_locked', retry_after_secs })).toBe(15)
  })
  it('the notice says "minute" for one and "minutes" otherwise', () => {
    expect(copy.login.locked(1)).toBe('Too many failed sign-ins. Try again in 1 minute.')
    expect(copy.login.locked(15)).toBe('Too many failed sign-ins. Try again in 15 minutes.')
  })
})
