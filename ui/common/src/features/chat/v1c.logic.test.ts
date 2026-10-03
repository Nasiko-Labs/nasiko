/**
 * Chat v1c M0 pure logic (plans/feat-chat-v1c.md §7): identity (test 1), copy guards (2), date groups
 * (3), the C2 storage (18), row tooltips (19), the preselect table (9), the TargetPicker's list, carrying
 * a draft with Undo (DS-T1, E15) and the mock variant lists (DX8).
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { Agent } from '@/features/agents/types'
import { clearStoredDrafts } from '@/lib/draftKeys'
import { CHAT_SCENARIOS } from '@/mocks/chat'
import { CHAT_PAGE_VARIANTS, MOCK_VARIANTS } from '@/mocks/handlers'
import { copy, errorCopy } from './copy'
import {
  carryDraft,
  clearDrafts,
  dropUndo,
  pendingUndo,
  readDraft,
  undoCarry,
  writeDraft,
} from './drafts'
import { chatKind } from './format'
import { chatIdentity, recordedName } from './identity'
import { dateGroupOf, groupByDate, rowTooltip } from './railGroups'
import { readRememberedTarget, rememberedTargetKey, rememberTarget } from './rememberTarget'
import { CHAT_PAGE_VARIANT_KEYS, CHAT_SCENARIO_KEYS } from './scenarioKeys'
import { mockEntries, unknownMockEntries } from './search'
import { orchestratorExamples, pickerAgents, preselectTarget } from './target'
import { tuning } from './tuning'
import type { ChatSessionRow } from './types'

const agent = (id: string, over: Partial<Agent> & Record<string, unknown> = {}): Agent =>
  ({
    id,
    name: `seed-${id}`,
    display_name: `Agent ${id}`,
    status: 'running',
    skills: [],
    tags: [],
    metadata: {},
    ...over,
  }) as unknown as Agent
const row = (over: Partial<ChatSessionRow>): ChatSessionRow => ({
  session_id: 's',
  agent_id: null,
  agent_url: null,
  title: 'T',
  created_at: '2026-03-20T10:00:00Z',
  ...over,
})

afterEach(() => {
  clearDrafts()
  vi.unstubAllGlobals()
})

describe('chatIdentity (test 1, §5.2)', () => {
  const dir = new Map([['a1', agent('a1', { status: 'stopped' })]])
  const rows = {
    direct: row({ agent_id: 'a1', agent_url: '/api/agents/a1', agent_name: 'seed-a1' }),
    rawFallback: row({ agent_id: 'zz', agent_url: '/api/agents/zz', agent_name: 'seed-zz' }),
    routedNull: row({ agent_id: null, agent_url: null }),
    routedUrl: row({ agent_id: null, agent_url: '/api/orchestrator/a2a' }),
    recorded: row({
      agent_id: 'h1',
      agent_url: '/api/agents/h1',
      agent_name: 'admin-claude-code',
      is_coding_agent: true,
    }),
    removed: row({ agent_id: null, agent_url: '/api/agents/gone' }),
  }

  it('names each kind and gives its subline', () => {
    expect(chatIdentity(rows.direct, dir, 'admin')).toMatchObject({
      kind: 'agent',
      name: 'Agent a1',
      railLabel: 'Agent a1',
      subline: 'Agent · Stopped',
      status: 'stopped',
      agentId: 'a1',
    })
    expect(chatIdentity(rows.rawFallback, dir, 'admin')).toMatchObject({
      kind: 'agent',
      name: 'seed-zz',
      status: undefined,
    })
    expect(chatIdentity(rows.routedNull, dir)).toMatchObject({
      kind: 'orchestrator',
      name: 'Orchestrator',
      subline: 'Picks agents for each message',
    })
    expect(chatIdentity(rows.routedUrl, dir)).toMatchObject({ kind: 'orchestrator' })
    expect(chatIdentity(rows.recorded, dir, 'admin')).toMatchObject({
      kind: 'recorded',
      name: 'claude-code',
      railLabel: 'Recorded',
      subline: 'Recorded from a coding harness',
    })
    expect(chatIdentity(rows.removed, dir)).toMatchObject({
      kind: 'removed',
      name: 'Agent removed',
      railLabel: 'Agent removed',
      subline: 'This agent no longer exists',
    })
  })

  it('agrees with chatKind on every row', () => {
    const kinds = {
      direct: 'agent',
      routed: 'orchestrator',
      recorded: 'recorded',
      removed: 'removed',
    } as const
    for (const r of Object.values(rows)) expect(chatIdentity(r, dir).kind).toBe(kinds[chatKind(r)])
  })

  it('strips only the viewer prefix from a recorded name, and shows the raw name until me loads', () => {
    expect(recordedName('admin-claude-code', 'admin')).toBe('claude-code')
    expect(recordedName('admin-claude-code')).toBe('admin-claude-code')
    expect(recordedName('bob-claude-code', 'admin')).toBe('bob-claude-code')
    expect(recordedName('admin-', 'admin')).toBe('admin-')
  })

  it('the picker never lists harness agents; running ones come first, by display name', () => {
    const { running, stopped } = pickerAgents([
      agent('b', { display_name: 'Beta' }),
      agent('a', { display_name: 'Alpha' }),
      agent('s', { display_name: 'Sleepy', status: 'stopped' }),
      agent('h1', { tags: ['coding-agent'] }),
      agent('h2', { metadata: { source: 'nasiko-cli-integration' } }),
      agent('h3', { is_coding_agent: true }),
    ])
    expect(running.map((a) => a.id)).toEqual(['a', 'b'])
    expect(stopped.map((a) => a.id)).toEqual(['s'])
  })
})

describe('copy guards (test 2, §5.1)', () => {
  const all = JSON.stringify({
    copy: Object.fromEntries(
      Object.entries(copy).map(([k, v]) => [
        k,
        typeof v === 'function' ? (v as (...a: unknown[]) => unknown)('x', 'y') : v,
      ]),
    ),
    errorCopy,
  })

  it('no "Auto" badge, no "Let OpenRuntime choose", and composed strings take the article', () => {
    expect(copy.orchestratorName).toBe('Orchestrator')
    expect(all).not.toContain('Let OpenRuntime choose')
    for (const banned of [
      /Ask Orchestrator/,
      /by Orchestrator/,
      /for Orchestrator/,
      /from Orchestrator/,
      /"Auto"/,
    ])
      expect(all).not.toMatch(banned)
    expect(copy.askOrchestrator).toBe('Ask the Orchestrator')
    expect(copy.theOrchestrator).toBe('the Orchestrator')
  })

  it('routed strings say Orchestrator; platform strings keep OpenRuntime', () => {
    for (const s of [
      copy.heroOrchestratorSubline,
      copy.routedBadgeTooltip,
      copy.answeredBy,
      copy.finishedWithoutReply,
      copy.announceNoReply,
      errorCopy.routedBadRequest.problem,
      errorCopy.routedForbidden.problem,
      errorCopy.routedFailed.problem,
    ])
      expect(s).toMatch(/Orchestrator/)
    for (const e of [
      errorCopy.createFailed,
      errorCopy.rateLimited,
      errorCopy.saveDefinite,
      errorCopy.saveUnknown,
      errorCopy.routedMayStillArrive,
      errorCopy.routedInternal,
    ])
      expect(`${e.problem} ${e.cause}`).toMatch(/OpenRuntime/)
  })
})

describe('date groups (test 3, §5.3, E9)', () => {
  const at = (iso: string) => ({ updated_at: iso, created_at: iso })

  it('splits at local midnight, not at UTC midnight', () => {
    const ny = 'America/New_York'
    const now = Date.parse('2026-03-20T04:10:00Z') // 00:10 in New York
    expect(dateGroupOf('2026-03-20T04:05:00Z', now, ny)).toBe('today')
    expect(dateGroupOf('2026-03-20T03:50:00Z', now, ny)).toBe('yesterday') // 23:50 the day before
    expect(dateGroupOf('2026-03-20T03:50:00Z', now, 'UTC')).toBe('today')
  })

  it('counts calendar days across a DST change', () => {
    const ny = 'America/New_York'
    // US clocks went forward on 2026-03-08; the 23-hour day still counts as one day.
    const now = Date.parse('2026-03-09T16:00:00Z')
    expect(dateGroupOf('2026-03-08T05:30:00Z', now, ny)).toBe('yesterday') // 00:30 on the 8th, before the change
    expect(dateGroupOf('2026-03-07T06:00:00Z', now, ny)).toBe('week')
    expect(dateGroupOf('2026-03-02T06:00:00Z', now, ny)).toBe('week')
    expect(dateGroupOf('2026-03-01T06:00:00Z', now, ny)).toBe('older')
    expect(dateGroupOf(undefined, now, ny)).toBe('older')
    expect(dateGroupOf('2026-03-10T06:00:00Z', now, ny)).toBe('today') // a clock ahead of ours
  })

  it('keeps row order, joins a Load more row to its own group, and leaves empty groups out', () => {
    const now = Date.parse('2026-03-20T15:00:00Z')
    const page1 = [
      { id: 'a', ...at('2026-03-20T14:00:00Z') },
      { id: 'b', ...at('2026-02-01T00:00:00Z') },
    ]
    const page2 = [{ id: 'c', ...at('2026-03-20T09:00:00Z') }]
    const groups = groupByDate([...page1, ...page2], now, 'UTC')
    expect(groups.map((g) => [g.group, g.rows.map((r) => r.id)])).toEqual([
      ['today', ['a', 'c']],
      ['older', ['b']],
    ])
    expect(groupByDate([], now, 'UTC')).toEqual([])
  })
})

describe('remembered target (test 18, C2)', () => {
  it('reads what was written, and every sign-out path clears it', () => {
    rememberTarget('u1', { kind: 'agent', id: 'a1' })
    expect(readRememberedTarget('u1')).toEqual({ kind: 'agent', id: 'a1' })
    rememberTarget('u1', { kind: 'orchestrator' })
    expect(readRememberedTarget('u1')).toEqual({ kind: 'orchestrator' })
    expect(localStorage.getItem(rememberedTargetKey('u1'))).toBe('orchestrator')
    clearDrafts('u1')
    expect(readRememberedTarget('u1')).toBeNull()
    rememberTarget('u2', { kind: 'orchestrator' })
    clearDrafts()
    expect(readRememberedTarget('u2')).toBeNull()
    rememberTarget('u3', { kind: 'orchestrator' })
    clearStoredDrafts()
    expect(readRememberedTarget('u3')).toBeNull()
  })

  it('blocked storage remembers nothing and never throws', () => {
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('blocked')
      },
      removeItem: () => undefined,
      key: () => null,
      length: 0,
    })
    expect(() => rememberTarget('u1', { kind: 'orchestrator' })).not.toThrow()
    expect(readRememberedTarget('u1')).toBeNull()
  })
})

describe('row tooltip (test 19, C3)', () => {
  it('is the title plus the first 120 characters of the last message as plain text', () => {
    const long = `**Bold** start ${'x'.repeat(200)}`
    const t = rowTooltip({ title: 'Deploy', last_message: `  ${long}\n\nmore` })
    const [title, preview] = t.split('\n')
    expect(title).toBe('Deploy')
    expect(preview).toHaveLength(120)
    expect(preview!.startsWith('Bold start')).toBe(true)
    expect(rowTooltip({ title: 'Only', last_message: null })).toBe('Only')
  })
})

describe('preselect (test 9 table, C2 and UC-A)', () => {
  const one = [
    agent('a'),
    agent('s', { status: 'stopped' }),
    agent('h', { tags: ['coding-agent'] }),
  ]
  const two = [agent('a'), agent('b')]
  it.each([
    [
      'remembered Orchestrator, directory failed',
      { kind: 'orchestrator' as const },
      undefined,
      false,
      { kind: 'routed' },
    ],
    [
      'remembered running agent, 2+ running',
      { kind: 'agent' as const, id: 'b' },
      two,
      false,
      { kind: 'agent', id: 'b' },
    ],
    [
      'remembered stopped agent, 2+ running',
      { kind: 'agent' as const, id: 's' },
      [...two, agent('s', { status: 'stopped' })],
      false,
      null,
    ],
    [
      'remembered missing agent, 2+ running',
      { kind: 'agent' as const, id: 'gone' },
      two,
      false,
      null,
    ],
    [
      'remembered stopped agent, 1 running',
      { kind: 'agent' as const, id: 's' },
      one,
      false,
      { kind: 'agent', id: 'a' },
    ],
    [
      'nothing remembered, 1 running (harness and stopped ignored)',
      null,
      one,
      false,
      { kind: 'agent', id: 'a' },
    ],
    ['nothing remembered, 2+ running', null, two, false, null],
    ['nothing remembered, 0 running', null, [agent('s', { status: 'stopped' })], false, null],
    ['typed before the directory loads', { kind: 'orchestrator' as const }, one, true, null],
    [
      'directory still loading, agent remembered',
      { kind: 'agent' as const, id: 'a' },
      undefined,
      false,
      null,
    ],
  ])('%s', (_name, remembered, agents, typed, expected) => {
    expect(preselectTarget({ remembered, agents, typed })).toEqual(expected)
  })
})

describe('Orchestrator examples (C1)', () => {
  it('one per running, non-harness agent, first string of its first skill that has one, de-duplicated, at most 3', () => {
    const skill = (examples: unknown[]) => ({
      id: 'k',
      name: 'k',
      description: '',
      tags: [],
      examples,
    })
    const got = orchestratorExamples([
      agent('a', { skills: [skill([]), skill([7, 'Summarise incidents'])] }),
      agent('b', { skills: [skill(['Summarise incidents'])] }),
      agent('s', { status: 'stopped', skills: [skill(['Stopped one'])] }),
      agent('h', { tags: ['coding-agent'], skills: [skill(['Harness one'])] }),
      agent('c', { skills: [skill(['Draft notes'])] }),
      agent('d', { skills: [skill(['Plan a migration'])] }),
      agent('e', { skills: [skill(['Too many'])] }),
    ])
    expect(got).toEqual([
      { agentId: 'a', text: 'Summarise incidents' },
      { agentId: 'c', text: 'Draft notes' },
      { agentId: 'd', text: 'Plan a migration' },
    ])
    expect(orchestratorExamples(undefined)).toEqual([])
  })
})

describe('carryDraft and Undo (DS-T1, E15, E16)', () => {
  it('moves the text on screen, keeps a displaced draft for Undo, and Undo swaps them back', () => {
    writeDraft('u', 'new:', 'typed')
    writeDraft('u', 'new:a', 'saved earlier')
    expect(carryDraft('u', 'new:', 'new:a', 1000)).toBe(true)
    expect(readDraft('u', 'new:a')).toBe('typed')
    expect(readDraft('u', 'new:')).toBe('')
    expect(pendingUndo('u', 'new:a', 1000)).toMatchObject({
      displaced: 'saved earlier',
      carried: 'typed',
    })
    expect(undoCarry('u', 2000)).toBe('saved earlier')
    expect(readDraft('u', 'new:a')).toBe('saved earlier')
    expect(readDraft('u', 'new:')).toBe('typed')
    expect(pendingUndo('u', 'new:a', 2000)).toBeNull()
  })

  it('A → B → C offers Undo for the last switch only; an edit or the time limit ends it', () => {
    writeDraft('u', 'new:a', 'text')
    writeDraft('u', 'new:b', 'b draft')
    writeDraft('u', 'new:c', 'c draft')
    carryDraft('u', 'new:a', 'new:b', 0)
    carryDraft('u', 'new:b', 'new:c', 0)
    expect(pendingUndo('u', 'new:b', 0)).toBeNull()
    expect(pendingUndo('u', 'new:c', 0)).toMatchObject({
      previousKey: 'new:b',
      displaced: 'c draft',
    })
    dropUndo('u')
    expect(undoCarry('u', 0)).toBeNull()
    carryDraft('u', 'new:c', 'new:a', 0)
    expect(readDraft('u', 'new:a')).toBe('text')
    writeDraft('u', 'new:b', 'again')
    carryDraft('u', 'new:a', 'new:b', 0)
    expect(pendingUndo('u', 'new:b', tuning.UNDO_DRAFT_MS)).toBeNull()
  })

  it("nothing typed moves nothing, so the target's saved draft loads as today", () => {
    writeDraft('u', 'new:a', 'saved')
    expect(carryDraft('u', 'new:', 'new:a')).toBe(false)
    expect(readDraft('u', 'new:a')).toBe('saved')
    expect(pendingUndo('u', 'new:a')).toBeNull()
  })
})

describe('mock variants (DX1, DX8)', () => {
  it('CHAT_PAGE_VARIANT_KEYS equals the chat subset of MOCK_VARIANTS and is apart from the stream scenarios', () => {
    expect([...CHAT_PAGE_VARIANT_KEYS].sort()).toEqual([...CHAT_PAGE_VARIANTS].sort())
    for (const k of CHAT_PAGE_VARIANT_KEYS) expect(MOCK_VARIANTS as readonly string[]).toContain(k)
    for (const k of CHAT_PAGE_VARIANT_KEYS) expect(Object.keys(CHAT_SCENARIOS)).not.toContain(k)
    for (const k of CHAT_PAGE_VARIANT_KEYS)
      expect(CHAT_SCENARIO_KEYS as readonly string[]).not.toContain(k)
  })

  it('?mock= is a comma list; the banner names each unknown entry', () => {
    expect(mockEntries(' many-chats, routed-plain ,,')).toEqual(['many-chats', 'routed-plain'])
    expect(unknownMockEntries('many-chats,routed-plain,nope,also-nope')).toEqual([
      'nope',
      'also-nope',
    ])
    expect(unknownMockEntries(undefined)).toEqual([])
  })

  it('a chat page variant never hides a degraded state listed after it', async () => {
    const was = location.href
    try {
      for (const q of [
        '?mock=many-chats,server-down',
        '?mock=no-agents,server-down',
        '?mock=server-down,many-chats',
      ]) {
        // eslint-disable-next-line no-restricted-globals -- test-only: sets the page URL the mock bootstrap reads `?mock=` from
        history.replaceState(null, '', q)
        // server-down fails /health like a stopped server: a network error.
        await expect(fetch(new URL('/health', location.origin)), q).rejects.toThrow()
      }
    } finally {
      // eslint-disable-next-line no-restricted-globals -- test-only: sets the page URL the mock bootstrap reads `?mock=` from
      history.replaceState(null, '', was)
    }
  })
})
