import { z } from 'zod/mini'
import { sharedSearchSchema } from '@nasiko/app-core/shell/context'
import { fallback, isRealDate, opt, PRESETS } from '@nasiko/ui/lib/search'

/** Sessions search params: the shared context plus the page's own (see app/shell/context.ts). */
export const sessionsSearchSchema = z.extend(sharedSearchSchema, {
  /** 7 days by default (TokenOps keeps 30d): the widest window session/list can search Tempo across. */
  preset: fallback(z.enum(PRESETS), '7d'),
  /** Day mode: that UTC day's sessions (the "follow the money" jump). */
  day: opt(z.string().check(z.refine(isRealDate))),
  lane: opt(z.enum(['failing', 'slow', 'costly'])),
  status: opt(z.enum(['failed', 'ok'])),
  sort: opt(z.enum(['cost', 'time'])),
  /** `paused` starts with Live off (and shows every row; no replay hold-back). */
  live: opt(z.enum(['paused'])),
})

export type SessionsSearch = z.infer<typeof sessionsSearchSchema>

export const traceSearchSchema = z.extend(sessionsSearchSchema, {
  trace: opt(z.string().check(z.trim(), z.minLength(1), z.maxLength(200))),
  /** HEX span id (what GET /span/{trace}/{span} matches), never the base64 `id`. */
  span: opt(z.string().check(z.regex(/^[0-9a-fA-F]{1,64}$/))),
  /** The harness session ledger (plans/feat-agent-centric.md PR1b): one row per turn instead of the trace. */
  view: opt(z.enum(['ledger'])),
})

export type TraceSearch = z.infer<typeof traceSearchSchema>
