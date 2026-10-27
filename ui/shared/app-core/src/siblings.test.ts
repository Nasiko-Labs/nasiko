import { describe, expect, it } from 'vitest'
import { siblingsAround } from './siblings'

describe('siblingsAround', () => {
  const list = ['a', 'b', 'c']

  it('places an id in its list', () => {
    expect(siblingsAround(list, 'b')).toEqual({
      siblings: list,
      index: 1,
      count: 3,
      prev: 'a',
      next: 'c',
    })
  })

  it('has no prev at the start and no next at the end', () => {
    expect(siblingsAround(list, 'a')).toMatchObject({ index: 0, prev: undefined, next: 'b' })
    expect(siblingsAround(list, 'c')).toMatchObject({ index: 2, prev: 'b', next: undefined })
  })

  it('is null on a deep link or for an id the list does not hold', () => {
    expect(siblingsAround(undefined, 'a')).toBeNull()
    expect(siblingsAround(list, 'z')).toBeNull()
  })
})
