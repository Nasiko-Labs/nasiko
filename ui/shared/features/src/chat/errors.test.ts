import { describe, expect, it } from 'vitest'
import { ApiError } from '@nasiko/data-access/client'
import { copy, errorCopy } from './copy'
import { copyDetails } from './copyDetails'
import {
  ChatError,
  dispatchError,
  failedTurnError,
  needsRunAgainConfirm,
  refusalOf,
  savedTurnError,
  restError,
  sendErrorText,
} from './errors'

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

describe('refusals a layer recognises', () => {
  const read = (detail: string) =>
    detail.includes('quota_hit')
      ? { problem: 'Quota hit.', cause: 'Refused.', action: 'Wait.', detail: 'Try tomorrow.' }
      : null

  it('asks the layer about a failed turn or a turn the server couldn’t start, and nothing else', () => {
    for (const key of ['agentFailed', 'routedFailed'] as const) {
      expect(refusalOf(failedTurnError(key, 'E: quota_hit'), read)?.detail).toBe('Try tomorrow.')
    }
    const unavailable = dispatchError(503, { error: { code: -32603, message: 'quota_hit' } })
    expect(refusalOf(unavailable, read)?.detail).toBe('Try tomorrow.')
    expect(refusalOf(failedTurnError('agentFailed', 'upstream timeout'), read)).toBeNull()
    const cut = new ChatError({
      phase: 'stream',
      key: 'cutOff',
      certainty: 'unknown',
      serverDetail: 'quota_hit',
    })
    expect(refusalOf(cut, read)).toBeNull()
  })

  it('an OSS build has no reader, so every failure is the generic one', () => {
    expect(refusalOf(failedTurnError('agentFailed', 'E: quota_hit'), null)).toBeNull()
    expect(failedTurnError('agentFailed', 'upstream timeout')).toMatchObject({
      key: 'agentFailed',
      certainty: 'unknown',
      serverDetail: 'upstream timeout',
    })
  })
})

describe('saved failures', () => {
  it('reads a saved failed turn back as the error the live turn showed', () => {
    const agent = savedTurnError({ turn_error: { message: 'upstream timeout', source: 'agent' } })
    expect(agent).toMatchObject({ key: 'agentFailed', serverDetail: 'upstream timeout' })
    const routed = savedTurnError({ turn_error: { message: 'model down', source: 'orchestrator' } })
    expect(routed?.key).toBe('routedFailed')
  })

  it('an ordinary reply is not a failure', () => {
    expect(savedTurnError(null)).toBeNull()
    expect(savedTurnError({ stopped: true })).toBeNull()
    expect(savedTurnError({ turn_error: 'nope' })).toBeNull()
  })
})

describe('dispatchError', () => {
  it.each([
    [400, 'badRequest', 'rejected-before-run'],
    [403, 'forbidden', 'rejected-before-run'],
    [404, 'agentGone', 'rejected-before-run'],
    [413, 'messageTooLarge', 'rejected-before-run'],
    [429, 'rateLimited', 'rejected-before-run'],
    // A 503 or 5xx that says why is the server's answer, shown with its reason (NAS-1624).
    [503, 'refusedWithReason', 'rejected-before-run'],
    [500, 'serverError', 'unknown'],
    [502, 'serverError', 'unknown'],
  ] as const)('%i → %s', (status, key, certainty) => {
    const e = dispatchError(status, {
      jsonrpc: '2.0',
      id: null,
      error: { code: -32603, message: 'boom' },
    })
    expect(e).toMatchObject({ key, certainty, rpcCode: -32603, serverDetail: 'boom' })
  })

  it('a 503 is "no running agent" only when the server says so or says nothing', () => {
    expect(dispatchError(503, null)).toMatchObject({ key: 'noAgents' })
    expect(
      dispatchError(503, { error: { code: -32603, message: 'no agents available' } }),
    ).toMatchObject({
      key: 'noAgents',
    })
    // a2a_dispatch.rs `Resolve` (-32609): not running, parked, workload down.
    expect(
      dispatchError(503, {
        error: { code: -32609, message: 'agent not running (status: stopped)' },
      }),
    ).toMatchObject({ key: 'noAgents', serverDetail: 'agent not running (status: stopped)' })
    expect(
      dispatchError(503, {
        error: { code: -32603, message: "attachment 'a.pdf' could not be read; try again" },
      }),
    ).toMatchObject({ key: 'refusedWithReason' })
  })

  it('a 5xx without a reason, or with the catch-all "internal error", is still a cut-off', () => {
    expect(dispatchError(500, null)).toMatchObject({ key: 'cutOff' })
    expect(
      dispatchError(500, { error: { code: -32603, message: 'internal error' } }),
    ).toMatchObject({
      key: 'cutOff',
      serverDetail: 'internal error',
    })
    expect(dispatchError(502, '<html>bad gateway</html>')).toMatchObject({
      key: 'cutOff',
      serverDetail: undefined,
    })
  })

  it('reads the reason from every body shape the server sends', () => {
    expect(dispatchError(500, { message: 'OPENAI_API_KEY not set' }).serverDetail).toBe(
      'OPENAI_API_KEY not set',
    )
    expect(dispatchError(500, { error: 'no key', code: 'llm_key_missing' }).serverDetail).toBe(
      'no key',
    )
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

describe('sendErrorText', () => {
  it("adds the server's reason as a sentence between the problem and the action", () => {
    const refused = dispatchError(503, {
      error: { code: -32603, message: "attachment 'a.pdf' could not be read; try again" },
    })
    expect(sendErrorText(refused)).toBe(
      `${errorCopy.refusedWithReason.problem} ${copy.serverSaid("attachment 'a.pdf' could not be read; try again.")} ${errorCopy.refusedWithReason.action}`,
    )
    // Without a reason it is the two-part line it always was.
    expect(sendErrorText(dispatchError(503, null))).toBe(
      `${errorCopy.noAgents.problem} ${errorCopy.noAgents.action}`,
    )
    expect(sendErrorText(null)).toBeNull()
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
  it('lists exactly phase, chat, trace, codes, the server reason, the doc anchor and the debug pointer', () => {
    const e = new ChatError({
      phase: 'dispatch',
      key: 'routedRateLimited',
      certainty: 'rejected-before-run',
      status: 429,
      serverDetail: 'rate limit exceeded',
    })
    const text = copyDetails(e, { sessionId: 's-1', traceId: null })
    expect(text).toBe(
      [
        'phase: dispatch',
        'chat: s-1',
        'trace: none',
        'http: 429',
        'rpc: none',
        'server: rate limit exceeded',
        'docs: docs/chat.md#errors-429',
        'more: reopen the chat with ?debug=turn',
      ].join('\n'),
    )
  })
})
