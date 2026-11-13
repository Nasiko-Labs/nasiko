import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { modelRouting } from '@nasiko/mocks/aggregate'
import type { ModelRoutingData } from '../types'
import { ModelRoutingCard } from './ModelRoutingCard'

const data = (over: Partial<ModelRoutingData> = {}): ModelRoutingData => ({
  ...modelRouting(),
  ...over,
})

describe('ModelRoutingCard', () => {
  it('names every level that served a call, and leaves out the ones that served none', () => {
    render(<ModelRoutingCard data={data()} loading={false} filtered={false} />)

    expect(screen.getByText('Small talk, served cheaply')).toBeInTheDocument()
    expect(screen.getByText('Classifier picked a tier')).toBeInTheDocument()
    // `pinned` and `default` served nothing in this window: a row of zeros would read as a level
    // that is somehow in play.
    expect(screen.queryByText('Pinned to one model')).not.toBeInTheDocument()
    expect(screen.queryByText('Default model')).not.toBeInTheDocument()
  })

  /** The whole point of the card: small talk is visible as its own share of the work. */
  it('shows each level’s share of the window’s calls', () => {
    render(<ModelRoutingCard data={data()} loading={false} filtered={false} />)

    const smallTalk = screen.getByRole('row', { name: /Small talk/ })
    expect(smallTalk).toHaveTextContent('54')
    expect(smallTalk).toHaveTextContent('12.9%')
  })

  it('marks a cost that covers only part of a level’s calls, and omits one it cannot attribute', () => {
    render(<ModelRoutingCard data={data()} loading={false} filtered={false} />)

    // The cache serves the rest of a classified turn, so most of its calls share a trace.
    expect(screen.getByRole('row', { name: /Reused the conversation/ })).toHaveTextContent('(part)')
    // Every classified call shared its trace with the cache hits after it: no cost of its own.
    const classified = screen.getByRole('row', { name: /Classifier picked a tier/ })
    expect(classified).not.toHaveTextContent('$')
  })

  /** A server older than the endpoint has no record either, so the card says nothing at all. */
  it('renders nothing when the endpoint is absent or nothing was routed', () => {
    const { container, rerender } = render(
      <ModelRoutingCard data={null} loading={false} filtered={false} />,
    )
    expect(container).toBeEmptyDOMElement()

    rerender(<ModelRoutingCard data={data({ calls: 0 })} loading={false} filtered={false} />)
    expect(container).toBeEmptyDOMElement()
  })

  it('says when the window has traces with no routing record', () => {
    render(<ModelRoutingCard data={data()} loading={false} filtered={false} />)
    expect(screen.getByText(/36 traces in this window carry no routing record/)).toBeInTheDocument()
  })
})
