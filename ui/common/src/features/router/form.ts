import type { z } from 'zod'

/**
 * Each field's first message from the sheet's zod schema, parsed synchronously. The sheets' zodResolver gates the
 * submit; these drive Save's disabled state and the inline messages on every render (RHF's own `errors` only fill
 * in after a change or a submit, and asynchronously).
 */
export function fieldErrors(schema: z.ZodType, values: unknown): Partial<Record<string, string>> {
  const r = schema.safeParse(values)
  const out: Partial<Record<string, string>> = {}
  if (!r.success) for (const i of r.error.issues) out[String(i.path[0])] ??= i.message
  return out
}
