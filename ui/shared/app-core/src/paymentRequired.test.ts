import { act, renderHook } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { ApiError } from '@nasiko/data-access/client'
import { usePaymentRequired } from './paymentRequired'
import { createQueryClient } from './queryClient'

const fail = (err: unknown) => {
  const client = createQueryClient(() => undefined)
  const run = client
    .getMutationCache()
    .build(client, { mutationFn: () => Promise.reject(err) })
    .execute(undefined)
  return { client, done: run.catch(() => undefined) }
}

describe('paymentRequired', () => {
  it('records a 402 from any mutation, with the server’s words, until the cache clears', async () => {
    const { result } = renderHook(() => usePaymentRequired())
    const { client, done } = fail(
      new ApiError(402, { code: 'paused', message: 'Add credits' }, '/api/mcp/connect', 'x'),
    )
    await act(() => done)
    expect(result.current).toEqual({
      path: '/api/mcp/connect',
      body: { code: 'paused', message: 'Add credits' },
      message: 'Add credits',
    })
    act(() => client.clear())
    expect(result.current).toBeNull()
  })

  it('records a 403 or 409 only when the server attached next steps', async () => {
    const { result } = renderHook(() => usePaymentRequired())
    await act(() => fail(new ApiError(409, { error: 'busy' }, '/api/x', 'x')).done)
    expect(result.current).toBeNull()
    const quota = { code: 'compute_quota_exceeded', error: 'Quota reached', actions: [] }
    await act(() => fail(new ApiError(403, quota, '/api/agents/upload', 'x')).done)
    expect(result.current?.body).toEqual(quota)
  })

  it('ignores every other failure', async () => {
    const { result } = renderHook(() => usePaymentRequired())
    await act(() => fail(new ApiError(403, 'no', '/api/x', 'x')).done)
    await act(() => fail(new Error('network')).done)
    expect(result.current).toBeNull()
  })
})
