// Routed end states and pauses each announce once (v1b §5.5, §8.1): StatusAnnouncer is the only
// live region, so a wrong mapping here fails silently for screen-reader users.
import { describe, expect, it } from 'vitest'
import { emptyTurn } from './a2aReducer'
import { copy } from './copy'
import { announcement } from './format'
import type { LiveTurn } from './turnRegistry'

const turn = (phase: LiveTurn['phase'], awaitingAgent?: string): LiveTurn =>
  ({
    id: 't',
    sessionId: 's',
    agentId: null,
    userText: 'x',
    phase,
    idle: false,
    stopped: false,
    finalized: false,
    error: null,
    saved: null,
    pendingSave: null,
    frames: [],
    chatMode: 'direct',
    operation: 'send',
    attempt: 'send',
    startedAt: 0,
    state: {
      ...emptyTurn(),
      ...(awaitingAgent ? { awaiting: { agent: awaitingAgent } } : {}),
    } as LiveTurn['state'],
  }) as LiveTurn

describe('routed announcements', () => {
  it('a routed pause names the agent that asked, else the chat agent', () => {
    expect(announcement(turn('streaming'), turn('paused', 'deployer'), 'OpenRuntime')).toBe(
      copy.announceRequest('deployer'),
    )
    expect(announcement(turn('streaming'), turn('paused'), 'OpenRuntime')).toBe(
      copy.announceRequest('OpenRuntime'),
    )
  })

  it('a chained pause (no awaiting_human) names askedBy when given', () => {
    expect(
      announcement(turn('streaming'), turn('paused'), copy.orchestratorName, 'Support Bot'),
    ).toBe(copy.announceRequest('Support Bot'))
  })

  it('awaiting_human still wins over askedBy, and without either it falls back to the chat agent', () => {
    expect(
      announcement(
        turn('streaming'),
        turn('paused', 'deployer'),
        copy.orchestratorName,
        'Support Bot',
      ),
    ).toBe(copy.announceRequest('deployer'))
    expect(announcement(turn('streaming'), turn('paused'), copy.orchestratorName)).toBe(
      copy.announceRequest(copy.orchestratorName),
    )
  })

  it.each([
    ['known_empty', copy.announceNoReply],
    ['lost', copy.announceUnconfirmed],
    ['resume_uncertain', copy.announceUnconfirmed],
    ['history_failed', copy.announceSavedFailed],
    ['not_started', copy.announceFailed],
    ['resume_forbidden', copy.announceFailed],
  ] as const)('%s says %s once', (phase, said) => {
    expect(announcement(turn('streaming'), turn(phase), 'OpenRuntime')).toBe(said)
    expect(announcement(turn(phase), turn(phase), 'OpenRuntime')).toBeNull()
  })
})
