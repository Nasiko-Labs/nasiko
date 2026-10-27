/**
 * Find a mocked agent across every `/api/agents` page (newest created first, 100 a page), so a test doesn't depend on
 * where the seed's creation dates put it.
 */
interface ListedAgent {
  id: string
  name: string
  tags?: string[]
  owner_id?: string
}

export async function agentWhere(test: (a: ListedAgent) => boolean): Promise<ListedAgent> {
  for (let offset = 0; ; offset += 100) {
    const page = (await (
      await fetch(`/api/agents?limit=100&offset=${offset}`)
    ).json()) as ListedAgent[]
    const hit = page.find(test)
    if (hit) return hit
    if (page.length < 100) throw new Error('no matching agent')
  }
}

export const agentNamed = (name: string) => agentWhere((a) => a.name === name)
