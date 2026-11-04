import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { recordRequestBodies } from '@nasiko/testing/setup'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'
import { sameSet } from './logic'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const T = { timeout: 12000 }
const c = copy.composio
const keyField = () => screen.findByLabelText(c.key.label, {}, T)
const saveButton = () => screen.getByRole('button', { name: copy.save })
const toolkit = (name: string) =>
  screen.getByRole('checkbox', { name: new RegExp(`^${name}`, 'i') })

describe('Settings: MCP toolkits (`?section=mcp`)', () => {
  it('starts with no key and every known toolkit off', async () => {
    renderApp('/settings?section=mcp')
    expect(await keyField()).toHaveValue('')
    expect(screen.getByText(c.key.status.none)).toBeInTheDocument()
    expect(toolkit('Slack')).not.toBeChecked()
    expect(screen.getByText(c.toolkits.count(0, 26))).toBeInTheDocument()
    expect(saveButton()).toBeDisabled()
  })

  it('will not switch a toolkit on until there is a key to register it with', async () => {
    renderApp('/settings?section=mcp')
    await keyField()
    await userEvent.click(toolkit('Slack'))
    expect(screen.getByText(c.toolkits.needsKey)).toBeInTheDocument()
    expect(saveButton()).toBeDisabled()
    await userEvent.type(screen.getByLabelText(c.key.label), 'ak_test')
    expect(screen.queryByText(c.toolkits.needsKey)).not.toBeInTheDocument()
    expect(saveButton()).toBeEnabled()
  })

  it('saves the key and the ticked toolkits, then never shows the key again', async () => {
    const bodies = recordRequestBodies()
    renderApp('/settings?section=mcp')
    await userEvent.type(await keyField(), 'ak_secret_value')
    await userEvent.click(toolkit('Slack'))
    await userEvent.click(toolkit('Gmail'))
    await userEvent.click(saveButton())
    expect(await screen.findByText(c.saved, {}, T)).toBeInTheDocument()
    await bodies.flush()
    const put = bodies.requests
      .filter((r) => r.method === 'PUT' && r.url.pathname === '/api/settings/composio')
      .at(-1)
    expect(put?.body).toEqual({
      api_key: 'ak_secret_value',
      toolkits: expect.arrayContaining(['slack', 'gmail']),
    })
    bodies.stop()
    // A fresh form on the saved state: the key box is empty, the status says one is saved, the ticks stuck.
    await waitFor(() => expect(screen.getByLabelText(c.key.label)).toHaveValue(''))
    expect(screen.getByText(c.key.status.ui)).toBeInTheDocument()
    expect(toolkit('Slack')).toBeChecked()
    expect(screen.queryByDisplayValue('ak_secret_value')).not.toBeInTheDocument()
    expect(screen.getByText(c.toolkits.count(2, 26))).toBeInTheDocument()
  })

  it('sends only what changed', async () => {
    const bodies = recordRequestBodies()
    renderApp('/settings?section=mcp')
    await userEvent.type(await keyField(), 'ak_first')
    await userEvent.click(saveButton())
    expect(await screen.findByText(c.saved, {}, T)).toBeInTheDocument()
    await bodies.flush()
    const put = bodies.requests.filter((r) => r.method === 'PUT').at(-1)
    expect(put?.body).toEqual({ api_key: 'ak_first' })
    bodies.stop()
  })

  it('search narrows the list, and Select all and Clear act on what is shown', async () => {
    renderApp('/settings?section=mcp')
    await userEvent.type(await keyField(), 'ak_test')
    await userEvent.type(screen.getByRole('searchbox', { name: c.toolkits.searchLabel }), 'google')
    expect(screen.getAllByRole('checkbox')).toHaveLength(3)
    await userEvent.click(screen.getByRole('button', { name: c.toolkits.selectAll }))
    expect(screen.getByText(c.toolkits.count(3, 26))).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: c.toolkits.clear }))
    expect(screen.getByText(c.toolkits.count(0, 26))).toBeInTheDocument()
    await userEvent.clear(screen.getByRole('searchbox', { name: c.toolkits.searchLabel }))
    await userEvent.type(screen.getByRole('searchbox', { name: c.toolkits.searchLabel }), 'zzz')
    expect(screen.getByText(c.toolkits.none)).toBeInTheDocument()
  })

  it('a removed key goes with the next save', async () => {
    const bodies = recordRequestBodies()
    renderApp('/settings?section=mcp')
    await userEvent.type(await keyField(), 'ak_first')
    await userEvent.click(saveButton())
    expect(await screen.findByText(c.saved, {}, T)).toBeInTheDocument()
    await userEvent.click(await screen.findByRole('button', { name: c.key.removeUi }))
    expect(screen.getByText(c.key.removing)).toBeInTheDocument()
    await userEvent.click(saveButton())
    await waitFor(() => expect(screen.getByText(c.key.status.none)).toBeInTheDocument(), T)
    await bodies.flush()
    expect(bodies.requests.filter((r) => r.method === 'PUT').at(-1)?.body).toEqual({
      clear_api_key: true,
    })
    bodies.stop()
  })

  it('is not offered to anyone but a superuser', async () => {
    configureMocks({ superuser: false })
    const { router } = renderApp('/settings?section=mcp')
    await waitFor(() => expect(router.state.location.pathname).toBe('/settings/secrets'), T)
    expect(screen.queryByLabelText(c.key.label)).not.toBeInTheDocument()
  })
})

describe('sameSet', () => {
  it('compares membership, not order', () => {
    expect(sameSet(new Set(['a', 'b']), new Set(['b', 'a']))).toBe(true)
    expect(sameSet(new Set(['a']), new Set(['a', 'b']))).toBe(false)
    expect(sameSet(new Set(['a', 'b']), new Set(['a', 'c']))).toBe(false)
  })
})
