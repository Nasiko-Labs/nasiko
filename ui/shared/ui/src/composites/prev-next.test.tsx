import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { TooltipProvider } from '@nasiko/ui/primitives/tooltip'
import { PrevNext } from './prev-next'

type Props = Partial<Parameters<typeof PrevNext>[0]>

function setup(props: Props = {}, id = 'a') {
  const onPrev = vi.fn()
  const onNext = vi.fn()
  const ui = (p: Props = {}, k = id) => (
    <TooltipProvider>
      <PrevNext
        key={k}
        noun="agent"
        index={1}
        count={3}
        onPrev={onPrev}
        onNext={onNext}
        {...props}
        {...p}
      />
    </TooltipProvider>
  )
  const view = render(ui())
  return {
    onPrev,
    onNext,
    rerender: (k: string) => view.rerender(ui({}, k)),
  }
}

describe('PrevNext', () => {
  it('shows the position and steps with the buttons', async () => {
    const { onPrev, onNext } = setup()
    expect(screen.getByRole('group', { name: 'agent 2 of 3' })).toBeInTheDocument()
    expect(screen.getByText('2 of 3')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Previous agent' }))
    await userEvent.click(screen.getByRole('button', { name: 'Next agent' }))
    expect(onPrev).toHaveBeenCalledExactlyOnceWith('click')
    expect(onNext).toHaveBeenCalledExactlyOnceWith('click')
  })

  it('steps with J and K', async () => {
    const { onPrev, onNext } = setup()
    await userEvent.keyboard('j')
    await userEvent.keyboard('k')
    expect(onNext).toHaveBeenCalledExactlyOnceWith('key')
    expect(onPrev).toHaveBeenCalledExactlyOnceWith('key')
  })

  it('stays focusable but inert at the ends', async () => {
    const { onPrev } = setup({ index: 0 })
    const prev = screen.getByRole('button', { name: 'Previous agent' })
    expect(prev).toHaveAttribute('aria-disabled', 'true')
    await userEvent.click(prev)
    await userEvent.keyboard('k')
    expect(onPrev).not.toHaveBeenCalled()
  })

  it('does nothing while blocked, and says why', async () => {
    const { onNext } = setup({ blocked: 'Save first' })
    const next = screen.getByRole('button', { name: 'Next agent' })
    expect(next).toHaveAttribute('aria-disabled', 'true')
    expect(next).toHaveAccessibleDescription('Save first')
    await userEvent.hover(next)
    expect((await screen.findAllByText('Save first')).length).toBeGreaterThan(0)
    await userEvent.click(next)
    await userEvent.keyboard('j')
    expect(onNext).not.toHaveBeenCalled()
  })

  it('keeps focus on the clicked arrow when the view remounts, but not a stale click', async () => {
    const { rerender } = setup()
    await userEvent.click(screen.getByRole('button', { name: 'Next agent' }))
    rerender('b')
    expect(screen.getByRole('button', { name: 'Next agent' })).toHaveFocus()
  })

  it('leaves focus alone after a remount when restoreFocus is off (the page focuses something better)', async () => {
    const { rerender } = setup({ restoreFocus: false })
    await userEvent.click(screen.getByRole('button', { name: 'Next agent' }))
    rerender('b')
    expect(screen.getByRole('button', { name: 'Next agent' })).not.toHaveFocus()
  })

  it('does not steal focus for a click older than 10 s', async () => {
    const { rerender } = setup()
    const now = Date.now()
    const spy = vi.spyOn(Date, 'now').mockReturnValue(now)
    await userEvent.click(screen.getByRole('button', { name: 'Next agent' }))
    spy.mockReturnValue(now + 11_000)
    rerender('b')
    expect(screen.getByRole('button', { name: 'Next agent' })).not.toHaveFocus()
    spy.mockRestore()
  })

  it('does not take focus from an element that already has it', async () => {
    const { rerender } = setup()
    await userEvent.click(screen.getByRole('button', { name: 'Next agent' }))
    const input = document.createElement('input')
    document.body.append(input)
    input.focus()
    rerender('b')
    expect(input).toHaveFocus()
    input.remove()
  })

  it('does not take focus while the clicked button is still mounted', async () => {
    setup()
    await userEvent.click(screen.getByRole('button', { name: 'Next agent' }))
    ;(document.activeElement as HTMLElement).blur()
    render(
      <TooltipProvider>
        <PrevNext noun="other" index={1} count={3} onPrev={vi.fn()} onNext={vi.fn()} />
      </TooltipProvider>,
    )
    expect(screen.getByRole('button', { name: 'Next other' })).not.toHaveFocus()
  })
})
