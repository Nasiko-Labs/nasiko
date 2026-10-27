/** Chat's query keys, in a leaf module so api.ts and sessionLookup.ts share them without a cycle. */
export const chatKeys = {
  all: ['chat'] as const,
  list: ['chat', 'list'] as const,
  history: (id: string) => ['chat', 'history', id] as const,
  flowsAll: ['chat', 'flows'] as const,
  flows: (traceId: string | null | undefined) => ['chat', 'flows', traceId] as const,
  lookup: (id: string | null | undefined) => ['chat', 'lookup', id] as const,
  /** Is this Sessions id a chat of mine? (v1c §5.10) */
  probe: (id: string) => ['chat', 'probe', id] as const,
  /** `GET /api/hitl/pending` for the Waiting queue (v1c §5.9), per user. */
  pendingAll: ['chat', 'pending'] as const,
  pending: (userId: string) => ['chat', 'pending', userId] as const,
}
