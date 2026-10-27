/**
 * The `?mock=` chat scenario names, kept apart from src/mocks so the app bundle never imports
 * mock code (bundle-boundary test). Must match `CHAT_SCENARIOS` in src/mocks/chat.ts
 * (components.test.tsx checks it).
 */
export const CHAT_SCENARIO_KEYS = [
  'direct-plain',
  'direct-steps',
  'terminal-usage-terminal',
  'a2a03-message',
  'a2a03-task',
  'a2a10-task',
  'lowercase-states',
  'append-reset',
  'multi-artifact',
  'hitl-options',
  'stream-garbage',
  'failed',
  'empty-reply',
  'routed-plain',
  'routed-multi-agent',
  'routed-no-tool',
  'routed-empty',
  'routed-sub-content-only',
  'routed-two-calls-one-turn',
  'routed-nested',
  'routed-hitl',
  'routed-hitl-truncated',
  'routed-hitl-no-reconnect',
  'routed-hitl-cancelled',
  'routed-hitl-expired',
  'routed-hitl-repeat',
  'routed-hitl-chained',
  'routed-agent-failed',
  'routed-policy-rejected',
  'routed-failed',
  'routed-oversized',
  'routed-malformed',
  'routed-cut',
  'routed-400',
  'routed-429',
  'routed-500',
  'routed-503',
  'routed-reconnect-400',
  'routed-reconnect-403',
  'create-slow',
  'direct-slow',
] as const

/**
 * The chat page variants (v1c DX1): page states that combine in one comma list with at most one stream
 * scenario, e.g. `?mock=many-chats,no-agents`. Must match `CHAT_PAGE_VARIANTS` in src/mocks/handlers.ts
 * and stay apart from the stream scenarios (components.test.tsx checks both).
 */
export const CHAT_PAGE_VARIANT_KEYS = [
  'no-agents',
  'many-chats',
  'many-recorded',
  'probe-500',
  'waiting',
  'pending-fail',
  'pending-flaky',
] as const
