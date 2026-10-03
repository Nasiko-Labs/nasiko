/** The model catalog query dedupes the server's overlapping open pricing rows (catalog.ts), as the mock returns them. */
import { describe, expect, it } from 'vitest'
import { apiFetch } from '@/lib/api/client'
import { createQueryClient } from '@/lib/queryClient'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { providersQuery } from './api'
import type { ProviderCatalogEntry } from './types'

setupPinnedSeed()

describe('providersQuery', () => {
  it('lists each model once per provider, keeping the newest pricing row', async () => {
    const raw = (await apiFetch<{ data: ProviderCatalogEntry[] }>('/api/llm-router/providers')).data
    const count = (groups: ProviderCatalogEntry[]) =>
      groups.flatMap((g) => g.models).filter((m) => m.model === 'gemini-2.0-flash').length
    expect(count(raw)).toBe(2)
    const groups = await createQueryClient(() => undefined, { retry: false }).fetchQuery(
      providersQuery,
    )
    expect(count(groups)).toBe(1)
    for (const g of groups) expect(new Set(g.models.map((m) => m.model)).size).toBe(g.models.length)
    expect(groups.flatMap((g) => g.models).find((m) => m.model === 'gemini-2.0-flash')?.notes).toBe(
      'gemini-2.0-flash',
    )
  })
})
