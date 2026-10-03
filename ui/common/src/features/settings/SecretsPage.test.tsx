import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { recordRequestBodies, recordRequests } from '@/test/setup'
import { renderApp } from '@/test/renderApp'
import { copy as all } from './copy'
import { REMASK_MS } from './SecretsPage'

const copy = all.secrets

setupPinnedSeed()
afterEach(() => {
  configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null })
  vi.unstubAllGlobals()
})

const T = { timeout: 5000 }
const row = async (name: string) =>
  (await screen.findByText(name, { selector: 'span' }, T)).closest('li') as HTMLElement

describe('Settings: secrets (plans/feat-settings.md §1.2)', () => {
  it('lists names masked, reads a value only when asked, and masks it again', async () => {
    const rec = recordRequests()
    renderApp('/settings/secrets')
    const r = await row('OPENAI_API_KEY')
    expect(within(r).getByText(copy.masked)).toBeInTheDocument()
    expect(rec.urls.some((u) => /^\/api\/secrets\/./.test(u.pathname))).toBe(false)
    await userEvent.click(within(r).getByRole('button', { name: copy.show('OPENAI_API_KEY') }))
    expect(await within(r).findByText('sk-mock-openai-api-key')).toBeInTheDocument()
    const hide = within(r).getByRole('button', { name: copy.hide('OPENAI_API_KEY') })
    expect(hide).toHaveAttribute('aria-pressed', 'true')
    await userEvent.click(hide)
    expect(within(r).getByText(copy.masked)).toBeInTheDocument()
    rec.stop()
  })

  it('a shown value masks itself after 30 s', async () => {
    renderApp('/settings/secrets')
    const r = await row('OPENAI_API_KEY')
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      await userEvent.click(within(r).getByRole('button', { name: copy.show('OPENAI_API_KEY') }))
      expect(await within(r).findByText('sk-mock-openai-api-key')).toBeInTheDocument()
      await vi.advanceTimersByTimeAsync(REMASK_MS)
      await waitFor(() => expect(within(r).getByText(copy.masked)).toBeInTheDocument())
    } finally {
      vi.useRealTimers()
    }
  })

  it('copies a value without showing it', async () => {
    const writeText = vi.fn(() => Promise.resolve())
    vi.stubGlobal('navigator', { ...navigator, clipboard: { writeText } })
    renderApp('/settings/secrets')
    const r = await row('ANTHROPIC_API_KEY')
    await userEvent.click(within(r).getByRole('button', { name: copy.copy('ANTHROPIC_API_KEY') }))
    expect(await screen.findByText(copy.copied('ANTHROPIC_API_KEY'), {}, T)).toBeInTheDocument()
    expect(writeText).toHaveBeenCalledWith('sk-mock-anthropic-api-key')
    expect(within(r).getByText(copy.masked)).toBeInTheDocument()
  })

  it('adds from the inline row after checking the name, then reveals what was saved', async () => {
    const bodies = recordRequestBodies()
    renderApp('/settings/secrets')
    const name = await screen.findByLabelText(copy.name, {}, T)
    await userEvent.type(name, 'api-key')
    await userEvent.type(screen.getByLabelText(copy.value), 'sk-123')
    await userEvent.click(screen.getByRole('button', { name: copy.add }))
    expect(await screen.findByText(copy.nameProblem.pattern)).toBeInTheDocument()
    await userEvent.clear(name)
    await userEvent.type(name, 'GEMINI_API_KEY')
    await userEvent.click(screen.getByRole('button', { name: copy.add }))
    expect(await screen.findByText(copy.saved('GEMINI_API_KEY'), {}, T)).toBeInTheDocument()
    // A fresh add row.
    await waitFor(() => expect(screen.getByLabelText(copy.name)).toHaveValue(''))
    const r = await row('GEMINI_API_KEY')
    await userEvent.click(within(r).getByRole('button', { name: copy.show('GEMINI_API_KEY') }))
    expect(await within(r).findByText('sk-123')).toBeInTheDocument()
    await bodies.flush()
    expect(
      bodies.requests
        .filter((q) => q.method === 'POST' && q.url.pathname === '/api/secrets')
        .map((q) => q.body),
    ).toEqual([{ name: 'GEMINI_API_KEY', value: 'sk-123' }])
    bodies.stop()
  })

  it('asks inline before deleting, naming the router configs that lose their key', async () => {
    renderApp('/settings/secrets')
    const r = await row('ANTHROPIC_API_KEY')
    await userEvent.click(within(r).getByRole('button', { name: copy.delete('ANTHROPIC_API_KEY') }))
    const confirm = await row('ANTHROPIC_API_KEY')
    expect(within(confirm).getByText(copy.confirm, { exact: false })).toBeInTheDocument()
    await waitFor(() =>
      expect(
        within(confirm).getByText(/anthropic-default will lose its provider key/),
      ).toBeInTheDocument(),
    )
    await userEvent.click(within(confirm).getByRole('button', { name: copy.cancel }))
    await userEvent.click(
      within(await row('ANTHROPIC_API_KEY')).getByRole('button', {
        name: copy.delete('ANTHROPIC_API_KEY'),
      }),
    )
    await userEvent.click(
      within(await row('ANTHROPIC_API_KEY')).getByRole('button', { name: copy.confirmDelete }),
    )
    expect(await screen.findByText(copy.deleted('ANTHROPIC_API_KEY'), {}, T)).toBeInTheDocument()
    await waitFor(() =>
      expect(screen.queryByText('ANTHROPIC_API_KEY', { selector: 'span' })).not.toBeInTheDocument(),
    )
  })

  it('a member manages their own secrets too', async () => {
    configureMocks({ superuser: false })
    renderApp('/settings/secrets')
    expect(await row('OPENAI_API_KEY')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: copy.add })).toBeInTheDocument()
  })
})
