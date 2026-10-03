/** The QueryClient's production retry policy: which errors retry, how long it waits, and mutations never. */
import { describe, expect, it } from 'vitest'
import { ApiError } from '@/lib/api/client'
import { createQueryClient } from './queryClient'

describe('createQueryClient retry policy (production settings)', () => {
  const retry = createQueryClient(() => undefined).getDefaultOptions().queries!.retry as (
    n: number,
    e: unknown,
  ) => boolean

  it('retries a 5xx, 408, 429, network error or deadline at most twice; never another 4xx, an abort or a bug', () => {
    expect(retry(0, new ApiError(500, null, '/p', 'm'))).toBe(true)
    expect(retry(1, new ApiError(502, null, '/p', 'm'))).toBe(true)
    expect(retry(2, new ApiError(500, null, '/p', 'm'))).toBe(false)
    expect(retry(0, new ApiError(429, null, '/p', 'm'))).toBe(true)
    expect(retry(0, new ApiError(408, null, '/p', 'm'))).toBe(true)
    expect(retry(0, new ApiError(404, null, '/p', 'm'))).toBe(false)
    expect(retry(0, new ApiError(403, null, '/p', 'm'))).toBe(false)
    expect(retry(0, new ApiError(401, null, '/p', 'm'))).toBe(false)
    expect(retry(0, new DOMException('slow', 'TimeoutError'))).toBe(true)
    expect(retry(0, new DOMException('gone', 'AbortError'))).toBe(false)
    expect(retry(0, new TypeError('x'))).toBe(false)
  })

  it("waits for a 429's Retry-After (capped at 30 s), else backs off exponentially", () => {
    const delay = createQueryClient(() => undefined).getDefaultOptions().queries!.retryDelay as (
      n: number,
      e: unknown,
    ) => number
    expect(delay(0, new ApiError(429, null, '/p', 'm', 5))).toBe(5000)
    expect(delay(0, new ApiError(429, null, '/p', 'm', 600))).toBe(30_000)
    expect(delay(0, new ApiError(500, null, '/p', 'm'))).toBe(1000)
    expect(delay(1, new ApiError(500, null, '/p', 'm'))).toBe(2000)
  })

  it('never retries a mutation', () => {
    expect(createQueryClient(() => undefined).getDefaultOptions().mutations!.retry).toBe(0)
  })
})
