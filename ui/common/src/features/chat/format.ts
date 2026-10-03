/** Small pure helpers for the Chat components (kept out of .tsx files for fast refresh). */
import { displayStatus, STATUS } from '@/features/agents/status'
import type { Agent } from '@/features/agents/types'
import { copy } from './copy'
import { splitStopped, toNumber } from './normalize'
import { isLivePhase, type LiveTurn } from './turnRegistry'
import type { ChatMessage, ChatSessionRow, UsageMeta } from './types'

const EXT: Record<string, string> = {
  bash: 'sh',
  sh: 'sh',
  shell: 'sh',
  zsh: 'sh',
  python: 'py',
  py: 'py',
  javascript: 'js',
  js: 'js',
  typescript: 'ts',
  ts: 'ts',
  tsx: 'tsx',
  jsx: 'jsx',
  json: 'json',
  yaml: 'yaml',
  yml: 'yaml',
  toml: 'toml',
  rust: 'rs',
  rs: 'rs',
  go: 'go',
  sql: 'sql',
  html: 'html',
  css: 'css',
  markdown: 'md',
  md: 'md',
  dockerfile: 'Dockerfile',
  java: 'java',
  ruby: 'rb',
  csv: 'csv',
}

/** Download filename for a code block: `snippet.<ext>` from the language, else `snippet.txt` (DS11). */
export function downloadName(lang: string): string {
  const ext = EXT[lang.toLowerCase()]
  if (ext === 'Dockerfile') return 'Dockerfile'
  return `snippet.${ext ?? 'txt'}`
}

/** Usage for one reply: from the live stream while streaming, else from the saved row. */
export interface TurnUsage {
  tokens: number | null
  cost: number | null
  durationMs: number | null
  model: string | null
  estimated: boolean
  inputTokens: number | null
  outputTokens: number | null
}

export function usageFromMeta(u: UsageMeta | null): TurnUsage | null {
  if (!u) return null
  const tokens =
    u.total_tokens ??
    (u.input_tokens !== undefined || u.output_tokens !== undefined
      ? (u.input_tokens ?? 0) + (u.output_tokens ?? 0)
      : null)
  return {
    tokens,
    cost: u.cost_usd ?? null,
    durationMs: u.duration_ms ?? null,
    model: u.model ?? null,
    estimated: u.estimated === true,
    inputTokens: u.input_tokens ?? null,
    outputTokens: u.output_tokens ?? null,
  }
}

export function usageFromMessage(m: ChatMessage): TurnUsage | null {
  const input = m.input_tokens ?? null
  const output = m.output_tokens ?? null
  const cost = toNumber(m.cost_usd)
  if (input === null && output === null && cost === null && m.duration_ms == null) return null
  return {
    tokens: input === null && output === null ? null : (input ?? 0) + (output ?? 0),
    cost,
    durationMs: m.duration_ms ?? null,
    model: m.model ?? null,
    estimated: m.usage_estimated === true,
    inputTokens: input,
    outputTokens: output,
  }
}

/** Header totals over loaded replies (E6): tokens, cost, and how many replies had no usage. */
export function chatTotals(replies: readonly ChatMessage[]): {
  tokens: number
  cost: number | null
  withoutUsage: number
  estimated: boolean
} {
  let tokens = 0
  let cost: number | null = null
  let withoutUsage = 0
  let estimated = false
  for (const m of replies) {
    const u = usageFromMessage(m)
    if (!u) {
      // quirk: §10.10 — resumed replies are saved without usage.
      withoutUsage++
      continue
    }
    tokens += u.tokens ?? 0
    if (u.cost !== null) cost = (cost ?? 0) + u.cost
    estimated ||= u.estimated
  }
  return { tokens, cost, withoutUsage, estimated }
}

/** A chat as Markdown, for Export (T-EXP). Stop markers become a readable note. */
export function exportMarkdown(title: string, messages: readonly ChatMessage[]): string {
  const lines = [`# ${title}`, '']
  for (const m of messages) {
    if (m.role === 'system') continue
    const { text, stopped } = splitStopped(m)
    lines.push(
      `## ${m.role === 'user' ? 'You' : 'Agent'} · ${m.timestamp}`,
      '',
      text,
      stopped ? '\n_(receiving stopped)_' : '',
      '',
    )
  }
  return lines.join('\n')
}

export type ChatKind = 'direct' | 'routed' | 'recorded' | 'removed'

/**
 * How Chat treats a chat (v1b plan §4, §2.8). A null `agent_id` is routed when the row says so:
 * `agent_url` is explicitly null (a routed create stores none) or the orchestrator. A null id
 * with an agent URL lost its agent, and an absent `agent_url` (the server always sends the key)
 * is never guessed as routed.
 */
export function chatKind(
  row: Pick<ChatSessionRow, 'agent_id' | 'agent_url' | 'is_coding_agent'>,
): ChatKind {
  if (row.is_coding_agent) return 'recorded'
  if (row.agent_id) return 'direct'
  if (
    row.agent_url === null ||
    (typeof row.agent_url === 'string' && row.agent_url.includes('/orchestrator'))
  )
    return 'routed'
  return 'removed'
}

export const agentLabel = (a: Pick<Agent, 'name' | 'display_name'>) =>
  a.display_name?.trim() || a.name

/** An agent's status in words, as the Agents pages say it (`status.ts`): "Running", "Stopped", "Needs attention"… */
export const statusLabel = (raw: string | null | undefined) =>
  STATUS[displayStatus(raw, false)].label

/** What StatusAnnouncer says when a turn changes phase (plan §8.1); null says nothing. */
export function announcement(
  prev: LiveTurn | undefined,
  next: LiveTurn | undefined,
  agent: string,
  askedBy?: string,
): string | null {
  if (!next || (prev && prev.id === next.id && prev.phase === next.phase)) return null
  switch (next.phase) {
    case 'done':
      return prev && isLivePhase(prev.phase) ? copy.announceReply(agent) : null
    // A routed pause names the agent that asked (awaiting_human), not OpenRuntime.
    // A chained pause has no awaiting_human: `askedBy` is the agent of the request being resumed.
    case 'paused':
      return copy.announceRequest(next.state.awaiting?.agent ?? askedBy ?? agent)
    case 'error':
    case 'no_reply':
    case 'unsaved':
    case 'not_started':
    case 'resume_forbidden':
      return copy.announceFailed
    // Routed end states (v1b §5.5): one announcement per outcome.
    case 'known_empty':
      return copy.announceNoReply
    case 'lost':
    case 'resume_uncertain':
      return copy.announceUnconfirmed
    case 'history_failed':
      return copy.announceSavedFailed
    default:
      return null
  }
}
