import { z } from 'zod/mini'
import { opt } from '@nasiko/ui/lib/search'

/** The core's workspace sections, in nav order (legacy settings-page.js TABS); a layer adds its own keys. */
export const CORE_SECTIONS = ['general', 'limits', 'registry'] as const
export type CoreSection = (typeof CORE_SECTIONS)[number]

/**
 * `/settings?section=`: a view of the page, replaced, not pushed. Any short string passes, since a layer's section
 * (EE `sso`) is a key the core doesn't know; the page falls back to General for one nobody serves.
 */
export const settingsSearchSchema = z.object({
  section: opt(z.string().check(z.maxLength(40))),
})
export type SettingsSearch = z.infer<typeof settingsSearchSchema>
