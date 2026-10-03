/**
 * The demo script's figures (docs/designs/openruntime-demo-script.md), pinned to its
 * anchor. If the seed, the mocks or a formatter change these numbers, this fails before
 * the script goes stale on stage.
 */
import { screen, waitFor, within } from '@testing-library/react'
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { SHOWCASE_SESSION } from '@/mocks/observability'
import { generateSeed } from '@/mocks/seed'
import { FIXED, seed as pinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'

const ANCHOR = new Date('2026-09-26T15:00:00Z')
const seed = generateSeed({ anchor: ANCHOR })

beforeAll(() => {
  vi.useFakeTimers({ toFake: ['Date'], now: ANCHOR })
  configureMocks({ seed, now: () => ANCHOR.getTime(), loggedIn: true })
})
afterAll(() => {
  vi.useRealTimers()
  configureMocks({ seed: pinnedSeed, now: () => FIXED.getTime() })
})

describe('demo script figures (anchor 2026-09-26)', () => {
  it('step 1 · TokenOps', async () => {
    renderApp('/tokenops')
    await waitFor(() =>
      expect(screen.getByTestId('summary-narrative')).toHaveTextContent(
        'In the last 30 days you spent at least $159.28, 31% less than the period before. Code Reviewer drove 33% of it.',
      ),
    )
    expect(
      screen.getByText('Spend peaked on Sep 17 at $18.12 (3.5× a typical day)'),
    ).toBeInTheDocument()
    expect(screen.getByRole('group', { name: 'Month progress' })).toHaveTextContent(
      '$141.69 spent this month',
    )
  })

  it('step 2 · Sessions on the spike day', async () => {
    renderApp('/sessions?day=2026-09-17&sort=cost')
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Sep 17 · 25 sessions · $18.12' }),
    ).toBeInTheDocument()
    const top = within(await screen.findByRole('list', { name: 'Sessions' })).getAllByRole(
      'link',
    )[0]
    expect(top).toHaveAccessibleName(
      /^Review PR #481 for race conditions · Code Reviewer · \$2\.63 · /,
    )
  })

  it('step 3 · the trace', { timeout: 20_000 }, async () => {
    renderApp(`/sessions/${SHOWCASE_SESSION}?day=2026-09-17&sort=cost`)
    expect(
      await screen.findByRole(
        'heading',
        { level: 1, name: /^Session · Code Reviewer · Sep 17 16:33 · \$2\.63 · 52 traces/ },
        { timeout: 8000 },
      ),
    ).toBeInTheDocument()
    expect(
      await screen.findByText('$0.10', { selector: 'span' }, { timeout: 8000 }),
    ).toBeInTheDocument()
    const story = screen.getByLabelText('What happened')
    await waitFor(
      () => expect(story).toHaveTextContent('This trace cost $0.10; retries cost $0.05.'),
      { timeout: 8000 },
    )
    expect(story).toHaveTextContent('It failed when the last attempt timed out at 30.0 s.')
  })
})
