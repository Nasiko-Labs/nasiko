/** Golden tests for the parity comparator (plans/feat-live-contract.md §7.1), one per rule in scripts/lib/shape.ts. */
import { describe, expect, it } from 'vitest'
import {
  applyAllowlist,
  compareResponses,
  compareShapes,
  compareValues,
  describe as describeDiff,
  shapeOf,
} from '../../../../scripts/lib/shape.ts'

const json = (body: unknown, status = 200) => ({ status, content_type: 'application/json', body })
const kinds = (live: unknown, mock: unknown, maps?: string[]) =>
  compareShapes(live, mock, maps).map((d) => `${d.kind} ${d.path}`)

describe('shapeOf', () => {
  it('records types per path and marks keys missing from some elements as absent', () => {
    const s = shapeOf({ rows: [{ a: 1, b: 'x' }, { a: null }] })
    expect([...s.types.get('$.rows[].a')!].sort()).toEqual(['null', 'number'])
    expect([...s.types.get('$.rows[].b')!].sort()).toEqual(['absent', 'string'])
  })
})

describe('compareShapes', () => {
  it('passes identical shapes with different values', () => {
    expect(kinds({ a: 1, b: [{ c: 'x' }] }, { a: 2, b: [{ c: 'y' }, { c: 'z' }] })).toEqual([])
  })
  it('reports a field the mock lacks, once, at its highest missing path', () => {
    expect(kinds({ a: { b: { c: 1 } } }, { x: 1 })).toEqual([
      'missing-in-mock $.a',
      'extra-in-mock $.x',
    ])
  })
  it('reports a renamed or retyped field', () => {
    expect(kinds({ total_cost: 1 }, { total_cost: '1' })).toEqual(['type $.total_cost'])
  })
  it('treats an empty array on either side as compatible with any element shape', () => {
    expect(kinds({ rows: [] }, { rows: [{ a: 1 }] })).toEqual([])
    expect(kinds({ rows: [{ a: 1 }] }, { rows: [] })).toEqual([])
  })
  it('keeps absent and null apart', () => {
    expect(kinds({ rows: [{ a: 1 }, {}] }, { rows: [{ a: 1 }, { a: null }] })).toEqual([
      'type $.rows[].a',
    ])
  })
  it('classifies null-only differences as nullability, not type', () => {
    expect(kinds({ a: null }, { a: 'x' })).toEqual(['nullability $.a'])
    expect(kinds({ a: 1 }, { a: 2 })).toEqual([])
    expect(kinds({ rows: [{ a: 1 }, { a: null }] }, { rows: [{ a: 2 }] })).toEqual([
      'nullability $.rows[].a',
    ])
  })
  it('collapses map_paths keys so data-keyed objects compare by value shape', () => {
    expect(
      kinds({ by_day: { '2026-09-01': 1 } }, { by_day: { '2026-09-02': 2 } }, ['$.by_day']),
    ).toEqual([])
    expect(kinds({ by_day: { '2026-09-01': 1 } }, { by_day: { '2026-09-02': 2 } })).toHaveLength(2)
  })
})

describe('compareResponses', () => {
  it('compares status, then content type, then shape', () => {
    expect(compareResponses(json({ a: 1 }, 404), json({ a: 1 }, 200)).map(describeDiff)).toEqual([
      '$: status live 404, mock 200',
    ])
  })
  it('compares string bodies by content type and emptiness, never by value', () => {
    expect(
      compareResponses(
        { status: 200, content_type: 'text/plain', body: 'ok' },
        { status: 200, content_type: 'text/plain', body: 'OK!' },
      ),
    ).toEqual([])
    expect(
      compareResponses(
        { status: 403, content_type: 'text/plain', body: 'requires admin role' },
        json({ error: 'x' }, 403),
      ).map((d) => d.kind),
    ).toEqual(['content-type', 'body-kind'])
    expect(
      compareResponses({ status: 404, content_type: '', body: '' }, json({ a: 1 }, 404)).map(
        (d) => d.kind,
      ),
    ).toEqual(['emptiness'])
  })
})

describe('applyAllowlist', () => {
  const diffs = compareShapes({ a: 1, b: { c: 1 } }, { x: 1 })
  it('explains differences at or below an allowlisted path', () => {
    const r = applyAllowlist(diffs, [
      { path: '$.a', reason: 'deliberate', since: '2026-09-28' },
      { path: '$.b', reason: 'deliberate', since: '2026-09-28' },
      { path: '$.x', reason: 'deliberate', since: '2026-09-28' },
    ])
    expect(r).toEqual({ unexplained: [], stale: [] })
  })
  it('reports an allowlist entry that explains nothing as stale', () => {
    const r = applyAllowlist(
      [],
      [{ path: '$.gone', reason: 'no longer differs', since: '2026-09-01' }],
    )
    expect(r.stale.map((a) => a.path)).toEqual(['$.gone'])
  })
})

describe('compareValues', () => {
  it('compares numbers within a tolerance and arrays by index', () => {
    expect(compareValues({ a: 1.0000001, b: [1, 2] }, { a: 1, b: [1, 2] })).toEqual([])
    expect(compareValues({ h: 17203.983885 }, { h: 17203.983893 })).toEqual([])
    expect(compareValues({ h: 17203.9 }, { h: 17204 })).toHaveLength(1)
    expect(compareValues({ b: [1, 2] }, { b: [2, 1] }).map((d) => d.path)).toEqual([
      '$.b[0]',
      '$.b[1]',
    ])
  })
  it('reports length and value differences', () => {
    expect(compareValues({ rows: [1, 2, 3] }, { rows: [1, 2] }).map((d) => d.path)).toEqual([
      '$.rows.length',
    ])
    expect(
      compareValues({ name: 'Invoice Parser' }, { name: 'seed-invoice-parser' }).map(describeDiff),
    ).toEqual(['$.name: value live "Invoice Parser", mock "seed-invoice-parser"'])
  })
  it('skips ignored paths at any index', () => {
    expect(
      compareValues(
        {
          rows: [
            { at: 1, v: 1 },
            { at: 2, v: 1 },
          ],
        },
        {
          rows: [
            { at: 9, v: 1 },
            { at: 8, v: 1 },
          ],
        },
        { ignore: ['$.rows[].at'] },
      ),
    ).toEqual([])
  })
  it('stops after the limit', () => {
    expect(compareValues({ r: [1, 2, 3, 4] }, { r: [5, 6, 7, 8] }, {}, 2)).toHaveLength(2)
  })
})
