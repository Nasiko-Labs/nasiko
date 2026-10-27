/**
 * The desktop app's "Forgot password?" probe (ForgotPassword.tsx): whether it can reset the server in use. It answers
 * `GET /__nasiko/local/reset/state` with `{desktop: true, resettable}` (ui/desktop-tauri/src-tauri/src/server.rs);
 * anywhere else that path is the server's 404 or the SPA's HTML.
 *
 * Read once per page load, outside the query cache: the answer can't change without a reload (the desktop app reloads
 * the window when it switches servers), and sign-out clears the cache while the sign-in page is mounting.
 */
import { z } from 'zod/mini'
import { apiFetch } from '@nasiko/data-access/client'

export const RESET_PAGE = '/__nasiko/local/reset'

const resetStateSchema = z.object({ desktop: z.literal(true), resettable: z.boolean() })

/** Every failure (404, HTML, another body, the network) is no. */
async function probeReset(): Promise<boolean> {
  try {
    const parsed = resetStateSchema.safeParse(await apiFetch<unknown>(`${RESET_PAGE}/state`))
    return parsed.success && parsed.data.resettable
  } catch {
    return false
  }
}

let probe: Promise<boolean> | null = null

/** This page load's answer, asked on first use. */
export function resettable(): Promise<boolean> {
  probe ??= probeReset()
  return probe
}

/** Tests: ask again on the next use, as a new page load would. */
export function forgetResetProbe() {
  probe = null
}
