// Regression: ISSUE-005 — the Flows routes dropped `?mock=flows-empty` / `flows-absent` / `trace-503` during search
// validation, so the documented mock variants never reached the mock.
// Found by /qa on 2026-10-06
// Report: .gstack/qa-reports/qa-report-localhost-2026-10-06.md
import { describe, expect, it } from 'vitest'
import { flowSearchSchema, flowsSearchSchema } from './search'

describe('Flows search keeps the mock variant (ISSUE-005)', () => {
  it('on the list and on a flow page, as written', () => {
    expect(flowsSearchSchema.parse({ mock: 'flows-empty' }).mock).toBe('flows-empty')
    expect(flowsSearchSchema.parse({ mock: 'flows-absent', kind: 'direct' })).toMatchObject({
      mock: 'flows-absent',
      kind: 'direct',
    })
    expect(flowSearchSchema.parse({ mock: 'trace-503', step: 'step:1' })).toEqual({
      mock: 'trace-503',
      step: 'step:1',
    })
    expect(flowsSearchSchema.parse({}).mock).toBeUndefined()
  })
})
