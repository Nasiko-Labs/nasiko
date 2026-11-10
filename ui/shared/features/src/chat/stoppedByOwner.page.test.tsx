/** NAS-1460 in the page: a new chat with a stopped agent shared with the viewer says its owner stopped it. */
import { screen } from '@testing-library/react'
import { afterEach, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { agentWhere } from '@nasiko/testing/agents'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, persona: null }))

it('a stopped agent someone else owns says its owner stopped it, not to start it', async () => {
  configureMocks({ seed, now, loggedIn: true, persona: 'sam' })
  const me = (await (await fetch('/api/me')).json()) as { sub: string }
  const stopped = await agentWhere((a) => a.status === 'stopped' && a.owner_id !== me.sub)
  renderApp(`/chat?agent=${stopped.id}`)
  // The banner and the composer's lock both say it.
  expect((await screen.findAllByText(/was stopped by its owner/)).length).toBeGreaterThan(0)
  expect(screen.queryByText(copy.targetNotRunning(stopped.name))).toBeNull()
})
