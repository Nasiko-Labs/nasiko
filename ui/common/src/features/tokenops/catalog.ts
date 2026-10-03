import type { ProviderCatalogEntry } from './types'

type Model = ProviderCatalogEntry['models'][number]

const from = (m: Model) =>
  m.effective_from ? Date.parse(m.effective_from) : Number.NEGATIVE_INFINITY

/**
 * One entry per model in each provider group. nasiko-server can return overlapping open pricing rows for one model
 * (llm_router/providers.rs at ea233d20: a `boot seed (static list)` row and a named row, both with no
 * `effective_until`), which the catalog would otherwise list twice. The row with the latest `effective_from` wins;
 * on a tie the first one does. Order is kept.
 */
export function dedupeCatalog(groups: readonly ProviderCatalogEntry[]): ProviderCatalogEntry[] {
  return groups.map((g) => {
    const best = new Map<string, Model>()
    for (const m of g.models) {
      const cur = best.get(m.model)
      if (!cur || from(m) > from(cur)) best.set(m.model, m)
    }
    const seen = new Set<string>()
    const models = g.models.filter(
      (m) => !seen.has(m.model) && best.get(m.model) === m && seen.add(m.model),
    )
    return models.length === g.models.length ? g : { ...g, models }
  })
}
