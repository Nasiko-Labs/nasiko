import { describe, expect, it } from 'vitest'
import { ApiError } from '@nasiko/data-access/client'
import { isObservabilityUnconfigured } from './unconfigured'

describe('isObservabilityUnconfigured', () => {
  it('is the server saying it has no trace store', () => {
    const body = 'observability backend not configured (set TEMPO_URL and LOKI_URL)'
    expect(isObservabilityUnconfigured(new ApiError(503, body, '/p', 'm'))).toBe(true)
  })

  it('is not any other 503, which must not be worded as a missing TEMPO_URL', () => {
    const body = "resource stats are not available for the 'kubernetes' runtime"
    expect(isObservabilityUnconfigured(new ApiError(503, body, '/p', 'm'))).toBe(false)
    expect(isObservabilityUnconfigured(new ApiError(503, '', '/p', 'm'))).toBe(false)
  })

  it('is not another status, or not an API error', () => {
    expect(isObservabilityUnconfigured(new ApiError(500, 'not configured', '/p', 'm'))).toBe(false)
    expect(isObservabilityUnconfigured(new Error('not configured'))).toBe(false)
  })
})
