import { describe, expect, it } from 'vitest'
import { ApiError } from '@/lib/api/client'
import { copy, errorCopy } from './copy'
import { copyDetails } from './copyDetails'
import { ChatError, dispatchError, needsRunAgainConfirm, restError } from './errors'

describe('error copy', () => {
  it('every key has a problem, a cause and an action', () => {
    for (const [key, c] of Object.entries(errorCopy)) {
      expect(c.problem.trim(), key).not.toBe('')
      expect(c.cause.trim(), key).not.toBe('')
      expect(c.action.trim(), key).not.toBe('')
    }
  })

  it('never uses retired or banned strings', () => {
    const all = JSON.stringify({ errorCopy, copy })
    for (const banned of [
      'control plane',
      'No response',
      '_Stopped_',
      'Stop and discard',
      'loaded turns',
    ])
      expect(all).not.toContain(banned)
  })
})

describe('dispatchError', () => {
  it.each([
    [400, 'badRequest', 'rejected-before-run'],
    [403, 'forbidden', 'rejected-before-run'],
    [404, 'agentGone', 'rejected-before-run'],
    [429, 'rateLimited', 'rejected-before-run'],
    [503, 'noAgents', 'rejected-before-run'],
    [500, 'cutOff', 'unknown'],
    [502, 'cutOff', 'unknown'],
  ] as const)('%i → %s', (status, key, certainty) => {
    const e = dispatchError(status, {
      jsonrpc: '2.0',
      id: null,
      error: { code: -32603, message: 'boom' },
    })
    expect(e).toMatchObject({ key, certainty, rpcCode: -32603, serverDetail: 'boom' })
  })

  it('keeps a plain-text 429 body as detail', () => {
    expect(dispatchError(429, 'rate limit exceeded, try again shortly').serverDetail).toBe(
      'rate limit exceeded, try again shortly',
    )
  })

  it('only unknown outcomes ask before Run again', () => {
    expect(needsRunAgainConfirm(dispatchError(500, null))).toBe(true)
    expect(needsRunAgainConfirm(dispatchError(429, null))).toBe(false)
  })
})

describe('restError', () => {
  it('splits assistant save failures into definite and unknown', () => {
    expect(restError('save-assistant', new ApiError(422, 'bad', '/x', 'test'))).toMatchObject({
      key: 'saveDefinite',
      certainty: 'definite',
    })
    expect(restError('save-assistant', new ApiError(502, null, '/x', 'test'))).toMatchObject({
      key: 'saveUnknown',
      certainty: 'unknown',
    })
    expect(restError('save-assistant', new TypeError('network'))).toMatchObject({
      key: 'saveUnknown',
    })
  })

  it('create and user-row failures dispatched nothing', () => {
    expect(restError('create', new ApiError(500, null, '/x', 'test')).certainty).toBe(
      'not-dispatched',
    )
    expect(restError('create', new ApiError(403, null, '/x', 'test')).key).toBe('forbidden')
    expect(restError('save-user', new ApiError(500, null, '/x', 'test'))).toMatchObject({
      key: 'saveUserFailed',
      certainty: 'not-dispatched',
    })
  })

  it('reads JSON-RPC error messages through ApiError', () => {
    expect(
      new ApiError(404, { error: { code: -32604, message: 'agent not found' } }, '/x', 'test')
        .serverMessage,
    ).toBe('agent not found')
  })
})

describe('Copy details (§5.6)', () => {
  it('lists exactly phase, chat, trace, codes, the doc anchor and the debug pointer', () => {
    const e = new ChatError({
      phase: 'dispatch',
      key: 'routedRateLimited',
      certainty: 'rejected-before-run',
      status: 429,
      serverDetail: 'secret body',
    })
    const text = copyDetails(e, { sessionId: 's-1', traceId: null })
    expect(text).toBe(
      [
        'phase: dispatch',
        'chat: s-1',
        'trace: none',
        'http: 429',
        'rpc: none',
        'docs: docs/chat.md#errors-429',
        'more: reopen the chat with ?debug=turn',
      ].join('\n'),
    )
    expect(text).not.toContain('secret')
  })
})
