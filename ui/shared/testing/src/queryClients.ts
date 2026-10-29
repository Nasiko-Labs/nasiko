import type { QueryClient } from '@tanstack/react-query'

/**
 * Every app QueryClient a test rendered. Unmounting the page does not stop its client: an in-flight fetch or a
 * polling query can still settle after the test, and Query's notify timer then reaches into React after the file's
 * jsdom is gone ("window is not defined", an unhandled error that fails the whole run). setup.ts cancels and clears
 * them after each test, while the environment is still up.
 */
const clients = new Set<QueryClient>()

export function trackQueryClient<T extends QueryClient>(client: T): T {
  clients.add(client)
  return client
}

export async function clearQueryClients() {
  const all = [...clients]
  clients.clear()
  for (const client of all) {
    await client.cancelQueries()
    client.clear()
  }
}
