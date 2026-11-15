import { screen, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()

describe('/cli Get started', () => {
  it('installs, then connects to this server, which signs in itself', async () => {
    renderApp('/cli')
    const start = await screen.findByRole('region', { name: copy.start.title })
    expect(
      within(start)
        .getAllByRole('code')
        .map((c) => c.textContent),
    ).toEqual([copy.install.unixCommand, `nasiko connect ${window.location.origin}`])
  })
})
