import { describe, expect, it } from 'vitest'
import { configureMocks } from './handlers'

const URL_ = new URL('/api/me/onboarding', globalThis.location.origin)
const patch = (persona: unknown) =>
  fetch(URL_, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ persona }),
  })

describe('onboarding mock (nasiko-cloud-rs 41f776ae onboarding.rs)', () => {
  it('answers a finished user by default in tests', async () => {
    const res = await fetch(URL_)
    expect(await res.json()).toEqual({ is_first_time_user: false, persona: 'developer' })
  })

  it('completes a first-time user on the first PATCH', async () => {
    configureMocks({ onboarding: { persona: null, completed: false } })
    expect(await (await fetch(URL_)).json()).toEqual({ is_first_time_user: true, persona: null })
    const res = await patch('finance')
    expect(await res.json()).toEqual({ is_first_time_user: false, persona: 'finance' })
    expect(await (await fetch(URL_)).json()).toEqual({
      is_first_time_user: false,
      persona: 'finance',
    })
  })

  it('rejects a persona outside the enum with a plain-text 422', async () => {
    const res = await patch('admin')
    expect(res.status).toBe(422)
    expect(await res.text()).toMatch(/unknown variant `admin`/)
  })

  it('answers a bare 404 when the route is absent', async () => {
    configureMocks({ variant: 'onboarding-absent' })
    const res = await fetch(URL_)
    expect(res.status).toBe(404)
    expect(await res.text()).toBe('')
    configureMocks({ variant: null })
  })
})
