import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
} from '@tanstack/react-router'
import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { TooltipProvider } from '@nasiko/ui/primitives/tooltip'
import { copy } from '../copy'
import { CARD_SKILLS } from '../tuning'
import type { Agent } from '../types'
import { AgentCard } from './AgentCard'

function agent(over: Partial<Agent> = {}): Agent {
  return {
    id: '0000beef-0000-4000-8000-000000000001',
    name: 'billing-bot',
    display_name: 'Billing Bot',
    description: 'Reconciles invoices.',
    status: 'running',
    owner_id: 'owner-1',
    version: '1.0.0',
    tags: [],
    skills: [],
    metadata: null,
    capabilities: {},
    default_input_modes: [],
    default_output_modes: [],
    preferred_transport: 'JSONRPC',
    protocol_version: '0.3',
    security_schemes: null,
    created_at: '2026-03-01T00:00:00Z',
    updated_at: '2026-03-01T00:00:00Z',
    ...over,
  } as Agent
}

function show(a: Agent, props: { meId?: string; ownerName?: string; reason?: string } = {}) {
  const root = createRootRoute({ component: Outlet })
  const index = createRoute({
    getParentRoute: () => root,
    path: '/',
    component: () => <AgentCard agent={a} {...props} />,
  })
  const detail = createRoute({
    getParentRoute: () => root,
    path: '/agents/$agentId',
    component: () => <p>Agent detail page</p>,
  })
  const router = createRouter({
    routeTree: root.addChildren([index, detail]),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  render(
    <TooltipProvider>
      <RouterProvider router={router} />
    </TooltipProvider>,
  )
  return router
}

const skills = (n: number) =>
  Array.from({ length: n }, (_, i) => ({ id: `s${i}`, name: `Skill ${i}`, description: '' }))

describe('AgentCard', () => {
  it('is one link to the agent page with the name, slug and description', async () => {
    show(agent())
    const link = await screen.findByRole('link')
    expect(link).toHaveAttribute('href', '/agents/0000beef-0000-4000-8000-000000000001')
    expect(within(link).getByText('Billing Bot')).toBeInTheDocument()
    expect(within(link).getByText('billing-bot')).toBeInTheDocument()
    expect(within(link).getByText('Reconciles invoices.')).toBeInTheDocument()
    expect(screen.getAllByRole('link')).toHaveLength(1)
  })

  it('opens the agent on click', async () => {
    const router = show(agent())
    await userEvent.click(await screen.findByRole('link'))
    await screen.findByText('Agent detail page')
    expect(router.state.location.pathname).toBe('/agents/0000beef-0000-4000-8000-000000000001')
  })

  it.each([[null], [''], [undefined]])(
    'names the card by its slug for display_name %j',
    async (display) => {
      show(agent({ display_name: display as string | null }))
      const link = await screen.findByRole('link')
      expect(link.querySelector('.truncate')).toHaveTextContent('billing-bot')
    },
  )

  it.each([[null], [''], [undefined]])(
    'says there is no description for %j',
    async (description) => {
      show(agent({ description: description as string | null }))
      expect(await screen.findByText(copy.noDescription)).toBeInTheDocument()
    },
  )

  it('keeps the full description as a title for the clamped line', async () => {
    const long = 'word '.repeat(200).trim()
    show(agent({ description: long }))
    expect(await screen.findByText(long)).toHaveAttribute('title', long)
  })

  it.each([
    ['running', 'Running'],
    ['crashed', 'Needs attention'],
    ['failed', 'Needs attention'],
    ['stopped', 'Stopped'],
    ['pending', 'Deploying'],
    ['registered', 'Not deployed'],
    ['strange', 'Unknown'],
  ])('shows raw status %s as %s', async (raw, label) => {
    show(agent({ status: raw }))
    expect(await screen.findByText(label)).toBeInTheDocument()
  })

  it('shows Yours for the owner and no owner name', async () => {
    show(agent({ owner_id: 'me' }), { meId: 'me', ownerName: 'Ada' })
    expect(await screen.findByText(copy.yours)).toBeInTheDocument()
    expect(screen.queryByText('Ada')).toBeNull()
  })

  it("shows Available to you and the owner name for someone else's agent", async () => {
    show(agent({ owner_id: 'other' }), { meId: 'me', ownerName: 'Ada' })
    expect(await screen.findByText(copy.availableToYou)).toBeInTheDocument()
    expect(screen.getByText('Ada')).toBeInTheDocument()
  })

  it('shows Available to you with no owner name when none is known', async () => {
    show(agent({ owner_id: 'other' }), { meId: 'me' })
    expect(await screen.findByText(copy.availableToYou)).toBeInTheDocument()
  })

  it('does not call an agent Yours when nobody is signed in', async () => {
    show(agent({ owner_id: 'other' }))
    expect(await screen.findByText(copy.availableToYou)).toBeInTheDocument()
    expect(screen.queryByText(copy.yours)).toBeNull()
  })

  it(`lists up to ${CARD_SKILLS} skills and counts the rest`, async () => {
    show(agent({ skills: skills(CARD_SKILLS + 4) }))
    await screen.findByRole('link')
    for (let i = 0; i < CARD_SKILLS; i++) expect(screen.getByText(`Skill ${i}`)).toBeInTheDocument()
    expect(screen.queryByText(`Skill ${CARD_SKILLS}`)).toBeNull()
    expect(screen.getByText('+4')).toBeInTheDocument()
  })

  it('shows exactly the skill limit with no +N', async () => {
    show(agent({ skills: skills(CARD_SKILLS) }))
    await screen.findByRole('link')
    expect(screen.queryByText(/^\+\d+$/)).toBeNull()
  })

  it('survives null skills', async () => {
    show(agent({ skills: null as unknown as Agent['skills'] }))
    expect(await screen.findByRole('link')).toBeInTheDocument()
  })

  it('marks a harness with its chip and no status badge or ownership chip', async () => {
    show(agent({ tags: ['coding-agent'], status: 'running' }))
    expect(await screen.findByText(copy.harnessChip)).toBeInTheDocument()
    expect(screen.queryByText('Running')).toBeNull()
    expect(screen.queryByText(copy.availableToYou)).toBeNull()
    expect(screen.queryByText(copy.yours)).toBeNull()
  })

  it('shows the health reason only when given', async () => {
    show(agent(), { reason: 'cost' })
    expect(await screen.findByTestId('health-reason')).toBeInTheDocument()
  })

  it('has no health reason line by default', async () => {
    show(agent())
    await screen.findByRole('link')
    expect(screen.queryByTestId('health-reason')).toBeNull()
  })
})
