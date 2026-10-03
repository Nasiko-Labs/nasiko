import { z } from 'zod'
import { sharedSearchSchema } from '@/app/shell/context'
import { isRealDate, PRESETS } from '@/lib/search'

/** Sessions search params: the shared context plus the page's own (see app/shell/context.ts). */
export const sessionsSearchSchema = sharedSearchSchema.extend({
  /** 7 days by default (TokenOps keeps 30d): the widest window session/list can search Tempo across. */
  preset: z.enum(PRESETS).default('7d').catch('7d'),
  /** Day mode: that UTC day's sessions (the "follow the money" jump). */
  day: z.string().refine(isRealDate).optional().catch(undefined),
  lane: z.enum(['failing', 'slow', 'costly']).optional().catch(undefined),
  status: z.enum(['failed', 'ok']).optional().catch(undefined),
  sort: z.enum(['cost', 'time']).optional().catch(undefined),
  /** `paused` starts with Live off (and shows every row; no replay hold-back). */
  live: z.enum(['paused']).optional().catch(undefined),
})

export type SessionsSearch = z.infer<typeof sessionsSearchSchema>

export const traceSearchSchema = sessionsSearchSchema.extend({
  trace: z.string().trim().min(1).max(200).optional().catch(undefined),
  /** HEX span id (what GET /span/{trace}/{span} matches), never the base64 `id`. */
  span: z
    .string()
    .regex(/^[0-9a-fA-F]{1,64}$/)
    .optional()
    .catch(undefined),
})

export type TraceSearch = z.infer<typeof traceSearchSchema>
