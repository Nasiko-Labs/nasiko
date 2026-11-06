/**
 * SuggestInput replaces `<datalist>` fields (the LLM router's models, onboarding's model): any text is accepted, a
 * suggestion fills the field, and inside a sheet Escape closes the list, not the sheet. While the list is open Base UI
 * hides the rest of the page from screen readers, so it never opens with nothing to show.
 */
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it } from 'vitest'
import { Sheet, SheetContent, SheetTitle } from '../primitives/sheet'
import { SuggestInput } from './suggest-input'

function Form({
  suggestions = ['claude-sonnet-4', 'gpt-4o', 'gpt-4o'],
}: {
  suggestions?: string[]
}) {
  const [v, setV] = useState('')
  const [open, setOpen] = useState(true)
  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetContent>
        <SheetTitle>Edit</SheetTitle>
        <label htmlFor="m">Model</label>
        <SuggestInput id="m" suggestions={suggestions} value={v} onValueChange={setV} />
        <button type="button">Save</button>
      </SheetContent>
    </Sheet>
  )
}

const options = () => screen.queryAllByRole('option').map((o) => o.textContent)

describe('SuggestInput', () => {
  it('suggests by substring, once per text, and a pick fills the field', async () => {
    const user = userEvent.setup()
    render(<Form />)
    const input = await screen.findByLabelText('Model')
    await user.type(input, 'gpt')
    await waitFor(() => expect(options()).toEqual(['gpt-4o']))
    await user.click(screen.getByRole('option', { name: 'gpt-4o' }))
    expect(input).toHaveValue('gpt-4o')
    await waitFor(() => expect(options()).toEqual([]))
  })

  it('takes any text, and Escape closes the list but not the sheet', async () => {
    const user = userEvent.setup()
    render(<Form />)
    const input = await screen.findByLabelText('Model')
    await user.type(input, 'claude')
    await screen.findByRole('option', { name: 'claude-sonnet-4' })
    await user.keyboard('{Escape}')
    await waitFor(() => expect(options()).toEqual([]))
    expect(screen.getByRole('dialog')).toBeInTheDocument()
    await user.type(input, '-next')
    expect(input).toHaveValue('claude-next')
  })

  it('keeps the list shut with nothing to suggest, so the rest of the sheet stays reachable', async () => {
    const user = userEvent.setup()
    render(<Form />)
    const input = await screen.findByLabelText('Model')
    await user.type(input, 'llama')
    expect(options()).toEqual([])
    expect(screen.getByRole('button', { name: 'Save' })).toBeInTheDocument()
  })

  it('Enter picks the highlighted suggestion; Tab moves on and closes the list', async () => {
    const user = userEvent.setup()
    render(<Form />)
    const input = await screen.findByLabelText('Model')
    await user.type(input, 'claude')
    await user.keyboard('{ArrowDown}')
    await waitFor(() => expect(input.getAttribute('aria-activedescendant')).toBeTruthy())
    await user.keyboard('{Enter}')
    expect(input).toHaveValue('claude-sonnet-4')
    await user.clear(input)
    await user.type(input, 'gpt')
    await screen.findByRole('option', { name: 'gpt-4o' })
    await user.tab()
    await waitFor(() => expect(options()).toEqual([]))
    expect(screen.getByRole('button', { name: 'Save' })).toBeInTheDocument()
  })
})
