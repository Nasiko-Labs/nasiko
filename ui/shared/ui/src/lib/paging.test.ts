import { describe, expect, it } from 'vitest'
import { pageCount, pageOf, pageWindow } from './paging'

describe('paging', () => {
  it('counts pages, never fewer than one', () => {
    expect(pageCount(0, 10)).toBe(1)
    expect(pageCount(10, 10)).toBe(1)
    expect(pageCount(11, 10)).toBe(2)
    expect(pageCount(23, 10)).toBe(3)
    expect(pageCount(23, 5)).toBe(5)
  })

  it('slices one page and clamps a page past either end', () => {
    const rows = Array.from({ length: 23 }, (_, i) => i)
    expect(pageOf(rows, 0, 10)).toEqual(rows.slice(0, 10))
    expect(pageOf(rows, 2, 10)).toEqual([20, 21, 22])
    expect(pageOf(rows, 9, 10)).toEqual([20, 21, 22])
    expect(pageOf(rows, -1, 10)).toEqual(rows.slice(0, 10))
    expect(pageOf(rows, 1, 5)).toEqual([5, 6, 7, 8, 9])
    expect(pageOf([], 0, 10)).toEqual([])
  })

  it('shows every page up to seven, then the ends and the pages around the current one', () => {
    expect(pageWindow(0, 1)).toEqual([0])
    expect(pageWindow(3, 7)).toEqual([0, 1, 2, 3, 4, 5, 6])
    expect(pageWindow(0, 8)).toEqual([0, 1, 'gap-end', 7])
    expect(pageWindow(0, 20)).toEqual([0, 1, 'gap-end', 19])
    expect(pageWindow(10, 20)).toEqual([0, 'gap-start', 9, 10, 11, 'gap-end', 19])
    expect(pageWindow(19, 20)).toEqual([0, 'gap-start', 18, 19])
    expect(pageWindow(2, 20)).toEqual([0, 1, 2, 3, 'gap-end', 19])
    expect(pageWindow(3, 20)).toEqual([0, 'gap-start', 2, 3, 4, 'gap-end', 19])
  })
})
