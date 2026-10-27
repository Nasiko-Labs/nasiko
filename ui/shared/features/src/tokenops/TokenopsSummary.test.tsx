import { screen, within, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'

setupPinnedSeed()

describe('TokenOps summary', () => {
  it('leads with the narrative, and every section is on the page with nothing to expand', async () => {
    renderApp('/tokenops')
    expect(await screen.findByTestId('summary-narrative')).toHaveTextContent(
      /^In the last 30 days you spent at least \$[\d.,]+, \d+% (more|less) than the period before\. Code Reviewer drove \d+% of it\.$/,
    )
    for (const heading of [
      'Spend over time',
      'This month',
      'Spend concentration',
      'Token mix',
      'Who is driving cost',
      'Cost vs performance',
      'Token optimization savings',
    ])
      expect(
        await screen.findByRole('heading', { name: new RegExp(`^${heading}`) }),
      ).toBeInTheDocument()
    expect(
      screen.queryAllByRole('button', { expanded: false, name: /^(Spend|Who|This|All)/ }),
    ).toEqual([])
    expect(screen.getByRole('group', { name: 'Month progress' })).toHaveTextContent(
      /spent this month/,
    )
  })

  it('lands on the optimization section when opened by link, not at the top of the page', async () => {
    // The regression this guards: the scroll ran before the sections had painted (they sit behind the page loader)
    // and nothing made it try again.
    //
    // Records `this.id` rather than using the spy's `mock.instances`, which is populated for `new`
    // calls and stays empty for a patched prototype method — asserting on it passes vacuously.
    const scrolled: string[] = []
    const original = Element.prototype.scrollIntoView
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this.id)
    }
    try {
      renderApp('/tokenops?open=optimize')
      await screen.findByRole('heading', { name: 'Token optimization savings' })
      await waitFor(() => expect(scrolled).toContain('tokenops-section-optimize'))
    } finally {
      Element.prototype.scrollIntoView = original
    }
  })

  it('the savings button lands exactly where the Overview link does, clear of the sticky rows', async () => {
    const scrolled: Element[] = []
    const original = Element.prototype.scrollIntoView
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this)
    }
    try {
      renderApp('/tokenops')
      const banner = await screen.findByRole('region', { name: /Optimization saved you/ })
      await userEvent.click(within(banner).getByRole('button', { name: /See the breakdown/ }))
      const target = scrolled.find((e) => e.id === 'tokenops-section-optimize')
      expect(target).toBeDefined()
      // A scroll margin under the section nav (the root's scroll-padding clears the top bar).
      expect(target!.className).toMatch(/scroll-mt-\d+/)
    } finally {
      Element.prototype.scrollIntoView = original
    }
  })

  it('puts the savings headline above the fold', async () => {
    renderApp('/tokenops')
    const banner = await screen.findByRole('region', { name: /Optimization saved you/ })
    expect(banner).toHaveTextContent(/fewer tokens/)
    expect(within(banner).getByRole('button', { name: /See the breakdown/ })).toBeInTheDocument()
  })

  it('the section nav jumps to a section and marks it current, without a history entry', async () => {
    const user = userEvent.setup()
    const scrolled = vi.spyOn(Element.prototype, 'scrollIntoView').mockImplementation(() => {})
    const { router } = renderApp('/tokenops')
    const nav = await screen.findByRole('navigation', { name: 'Sections' })
    const before = router.state.location.state.__TSR_index
    await user.click(within(nav).getByRole('button', { name: /^Drivers/ }))
    expect(scrolled.mock.contexts.map((e) => (e as Element).id)).toContain(
      'tokenops-section-drivers',
    )
    expect(within(nav).getByRole('button', { name: /^Drivers/ })).toHaveAttribute(
      'aria-current',
      'true',
    )
    expect(within(nav).getByRole('button', { name: /^Spend/ })).not.toHaveAttribute('aria-current')
    expect(router.state.location.state.__TSR_index).toBe(before)
    scrolled.mockRestore()
  })

  it('older links still land: ?open=month and ?open=perf go to the sections that now hold them', async () => {
    const scrolled = vi.spyOn(Element.prototype, 'scrollIntoView').mockImplementation(() => {})
    renderApp('/tokenops?open=month')
    await waitFor(() =>
      expect(scrolled.mock.contexts.map((e) => (e as Element).id)).toContain(
        'tokenops-section-spend',
      ),
    )
    scrolled.mockRestore()
  })

  it("a day drill-down offers that day's sessions", async () => {
    renderApp(`/tokenops?day=${seed.spikeDate}`)
    const links = await screen.findAllByRole('link', { name: /See sessions/ })
    expect(links.some((l) => l.getAttribute('href')?.includes(`day=${seed.spikeDate}`))).toBe(true)
  })

  it('Compare off: no change clause, no Δ in the figures or KPIs', async () => {
    renderApp('/tokenops?compare=0')
    await waitFor(() =>
      expect(screen.getByTestId('summary-narrative')).toHaveTextContent(
        /^In the last 30 days you spent at least \$[\d.,]+\. /,
      ),
    )
    expect(screen.getByText('Compare is off')).toBeInTheDocument()
    expect(
      (await screen.findAllByLabelText('comparison unavailable')).length,
    ).toBeGreaterThanOrEqual(4)
  })
})
