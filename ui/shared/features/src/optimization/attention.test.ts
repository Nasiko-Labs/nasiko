/** Needs attention's split (plans/feat-optimization-page.md R2F, eng E2). */
import { describe, expect, it } from 'vitest'
import { attentionLines, shownNames, SHOWN_NAMES } from './attention'

const row = (id: string, owner: string, on: boolean, name = id) => ({
  id,
  name,
  owner_id: owner,
  compress_enabled: on,
})

describe('attentionLines', () => {
  const owned = [
    row('b', 'me', false, 'Bravo'),
    row('a', 'me', false, 'Alpha'),
    row('c', 'me', true),
  ]
  const directory = [
    ...owned,
    row('x', 'ann', false),
    row('y', 'bob', true),
    row('z', 'bob', false),
  ]

  it('line 1 is your own off agents, sorted by name; line 2 other owners’ for superusers only', () => {
    const su = attentionLines({ owned, directory, me: 'me', superuser: true, hidden: new Set() })
    expect(su.mine.map((a) => a.name)).toEqual(['Alpha', 'Bravo'])
    expect(su.others.map((a) => a.id)).toEqual(['x', 'z'])
    const member = attentionLines({
      owned,
      directory,
      me: 'me',
      superuser: false,
      hidden: new Set(),
    })
    expect(member.others).toEqual([])
  })

  it('a row without the flag is unknown, never a bulk target; turned-on ids drop off at once', () => {
    const r = attentionLines({
      owned: [{ id: 'q', name: 'q', owner_id: 'me' }, row('a', 'me', false), row('b', 'me', false)],
      directory: [{ id: 'z', name: 'z', owner_id: 'ann' }],
      me: 'me',
      superuser: true,
      hidden: new Set(['a']),
    })
    expect(r.mine.map((a) => a.id)).toEqual(['b'])
    expect(r.others).toEqual([])
  })
})

describe('shownNames', () => {
  it('shows every name up to one past the cap, else the cap and a count', () => {
    const list = Array.from({ length: SHOWN_NAMES + 1 }, (_, i) => i)
    expect(shownNames(list)).toEqual({ shown: list, more: 0 })
    expect(shownNames([...list, 99])).toMatchObject({ more: 2 })
  })
})
