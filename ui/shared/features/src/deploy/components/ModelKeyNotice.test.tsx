/**
 * The Deploy page's model-key notice says the layer's `deployNote` in place of its own line only where the Starter pack is
 * offered (MT: the Free plan's allowance for deployed agents), never on a workspace the slot turns it off for, and never
 * the pack's own note.
 */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { describe, expect, it } from 'vitest'
import { EditionProvider } from '@nasiko/app-core/edition/EditionProvider'
import { starterPack, type Edition } from '@nasiko/app-core/edition/edition'
import { copy } from '../copy'
import { ModelKeyNotice } from './ModelKeyNotice'

const NOTE = 'Agents you deploy get 50 model calls a day.'
const PACK_NOTE = 'Starter pack agents get 50 model calls a day.'

/** An edition whose one layer answers the Starter pack with `free`, carrying both notes. */
const editionAnswering = (free: boolean): Edition => ({
  id: 'test',
  layers: [
    {
      id: 'test',
      slots: {
        starterPack: starterPack({
          query: { queryKey: ['test', 'starter-pack'], queryFn: () => Promise.resolve(free) },
          offered: (f) => f !== false,
          note: PACK_NOTE,
          deployNote: NOTE,
        }),
      },
    },
  ],
})

function renderNotice(edition: Edition) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const router = createRouter({
    routeTree: createRootRoute({ component: ModelKeyNotice }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  render(
    <EditionProvider edition={edition}>
      <QueryClientProvider client={qc}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </EditionProvider>,
  )
  return qc
}

describe('ModelKeyNotice', () => {
  it("says the layer's deploy note in place of its own line where the pack is offered", async () => {
    renderNotice(editionAnswering(true))
    expect(await screen.findByText(NOTE)).toBeInTheDocument()
    expect(screen.queryByText(copy.keyNotice.body)).toBeNull()
    expect(screen.queryByText(PACK_NOTE)).toBeNull()
  })

  it('keeps its own line where the slot turns the pack off', async () => {
    const qc = renderNotice(editionAnswering(false))
    expect(await screen.findByText(copy.keyNotice.body)).toBeInTheDocument()
    // The slot answers after the first render: wait for its answer before saying the note never shows.
    await waitFor(() => expect(qc.getQueryState(['test', 'starter-pack'])?.status).toBe('success'))
    expect(screen.queryByText(NOTE)).toBeNull()
  })
})
