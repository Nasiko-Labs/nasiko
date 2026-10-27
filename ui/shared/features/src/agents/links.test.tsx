/**
 * E2 (plan §4/§5, §10): agent names in Sessions, the session trace and TokenOps link to the
 * agent's detail page through the shared directory; an agent the directory doesn't have
 * renders as plain text. (Harnesses shows developers and harnesses, never agent names.)
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { observabilityData, SHOWCASE_SESSION } from '@nasiko/mocks/observability'
import { seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'

setupPinnedSeed()

const data = observabilityData(seed)
const agentLinks = () =>
  screen
    .queryAllByRole('link')
    .filter((l) => /^\/agents\/[0-9a-f-]{36}$/.test(l.getAttribute('href') ?? ''))
const emptyDirectory = () => server.use(http.get('/api/agents', () => HttpResponse.json([])))

describe('Sessions', () => {
  /** The first row of a live agent (the seed also has sessions of a deleted one). */
  const expandFirst = async () => {
    const toggles = await screen.findAllByRole('button', { name: /^Show details for / })
    await userEvent.click(
      toggles.find((t) => !t.closest('li')!.textContent!.includes('(unknown agent)'))!,
    )
  }

  // The agent link ("Open <agent>"), not the row's Open chat / Open flow links (plans/feat-flows.md F19).
  const AGENT_LINK = /^Open (?!chat$|flow$|latest flow$)/

  it('an expanded row links to its agent', async () => {
    renderApp('/sessions?live=paused')
    await expandFirst()
    const link = await screen.findByRole('link', { name: AGENT_LINK })
    expect(link.getAttribute('href')).toMatch(/^\/agents\/[0-9a-f-]{36}$/)
  })

  it('no link when the agent is not in the directory', async () => {
    emptyDirectory()
    renderApp('/sessions?live=paused')
    await expandFirst()
    await screen.findByRole('button', { name: /logs$/ })
    expect(screen.queryByRole('link', { name: AGENT_LINK })).toBeNull()
  })
})

describe('Session trace', () => {
  const heading = () => screen.findByText(/^Session ·/)

  it('the header names the agent as a link', async () => {
    renderApp(`/sessions/${SHOWCASE_SESSION}?trace=${data.showcaseTraceId}`)
    const line = await heading()
    await waitFor(() =>
      expect(within(line).getByRole('link')).toHaveAttribute(
        'href',
        expect.stringMatching(/^\/agents\/[0-9a-f-]{36}$/),
      ),
    )
  })

  it('plain text when the agent is unknown', async () => {
    emptyDirectory()
    renderApp(`/sessions/${SHOWCASE_SESSION}?trace=${data.showcaseTraceId}`)
    const line = await heading()
    expect(within(line).queryByRole('link')).toBeNull()
  })
})

describe('TokenOps', () => {
  it('attribution rows link to their agents', async () => {
    renderApp('/tokenops?open=all')
    await screen.findAllByRole('button', { name: /^View traces for / })
    await waitFor(() => expect(agentLinks().length).toBeGreaterThan(0))
  })

  it('no agent links when the directory is empty', async () => {
    emptyDirectory()
    renderApp('/tokenops?open=all')
    await screen.findAllByRole('button', { name: /^View traces for / })
    expect(agentLinks()).toHaveLength(0)
  })
})
