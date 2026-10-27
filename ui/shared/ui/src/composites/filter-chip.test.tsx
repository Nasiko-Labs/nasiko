import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { FilterChip } from './filter-chip'

function Controlled({ onClick }: { onClick?: () => void }) {
  const [on, setOn] = useState(false)
  return (
    <FilterChip
      active={on}
      onClick={() => {
        onClick?.()
        setOn(!on)
      }}
    >
      Running
    </FilterChip>
  )
}

describe('FilterChip', () => {
  it('is a toggle button named by its children', () => {
    render(
      <FilterChip active={false} onClick={() => undefined}>
        Ops
      </FilterChip>,
    )
    expect(screen.getByRole('button', { name: 'Ops' })).toBeInTheDocument()
  })

  it.each([
    [true, 'true', 'on'],
    [false, 'false', 'off'],
  ])('shows active=%s as aria-pressed=%s and data-state=%s', (active, pressed, state) => {
    render(
      <FilterChip active={active} onClick={() => undefined}>
        Ops
      </FilterChip>,
    )
    const chip = screen.getByRole('button', { name: 'Ops' })
    expect(chip).toHaveAttribute('aria-pressed', pressed)
    expect(chip).toHaveAttribute('data-state', state)
  })

  it('calls onClick once per press when switching on', async () => {
    const onClick = vi.fn()
    render(
      <FilterChip active={false} onClick={onClick}>
        Ops
      </FilterChip>,
    )
    await userEvent.click(screen.getByRole('button', { name: 'Ops' }))
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it('calls onClick once per press when switching off', async () => {
    const onClick = vi.fn()
    render(
      <FilterChip active onClick={onClick}>
        Ops
      </FilterChip>,
    )
    await userEvent.click(screen.getByRole('button', { name: 'Ops' }))
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it('takes its pressed state from active, not from its own clicks', async () => {
    render(
      <FilterChip active={false} onClick={() => undefined}>
        Ops
      </FilterChip>,
    )
    const chip = screen.getByRole('button', { name: 'Ops' })
    await userEvent.click(chip)
    expect(chip).toHaveAttribute('aria-pressed', 'false')
  })

  it('toggles on and off with a parent that owns the state', async () => {
    const onClick = vi.fn()
    render(<Controlled onClick={onClick} />)
    const chip = screen.getByRole('button', { name: 'Running' })
    await userEvent.click(chip)
    expect(chip).toHaveAttribute('aria-pressed', 'true')
    await userEvent.click(chip)
    expect(chip).toHaveAttribute('aria-pressed', 'false')
    expect(onClick).toHaveBeenCalledTimes(2)
  })

  it('works from the keyboard with Space and Enter', async () => {
    const onClick = vi.fn()
    render(<Controlled onClick={onClick} />)
    await userEvent.tab()
    expect(screen.getByRole('button', { name: 'Running' })).toHaveFocus()
    await userEvent.keyboard(' ')
    await userEvent.keyboard('{Enter}')
    expect(onClick).toHaveBeenCalledTimes(2)
  })

  it('takes rich children: a label and a count', () => {
    render(
      <FilterChip active={false} onClick={() => undefined}>
        Running<span>12</span>
      </FilterChip>,
    )
    expect(screen.getByRole('button', { name: 'Running12' })).toBeInTheDocument()
  })

  it('takes a number as its child', () => {
    render(
      <FilterChip active={false} onClick={() => undefined}>
        {7}
      </FilterChip>,
    )
    expect(screen.getByRole('button', { name: '7' })).toBeInTheDocument()
  })

  it('keeps a very long label in one button', () => {
    const long = 'tag'.repeat(200)
    render(
      <FilterChip active={false} onClick={() => undefined}>
        {long}
      </FilterChip>,
    )
    expect(screen.getByRole('button', { name: long })).toBeInTheDocument()
  })

  it('does not call onClick on render or on hover', async () => {
    const onClick = vi.fn()
    render(
      <FilterChip active onClick={onClick}>
        Ops
      </FilterChip>,
    )
    await userEvent.hover(screen.getByRole('button', { name: 'Ops' }))
    expect(onClick).not.toHaveBeenCalled()
  })

  it('keeps the pill styling the catalogs rely on', () => {
    render(
      <FilterChip active onClick={() => undefined}>
        Ops
      </FilterChip>,
    )
    const chip = screen.getByRole('button', { name: 'Ops' })
    expect(chip.className).toContain('rounded-full')
    expect(chip.className).toContain('min-h-8')
    expect(chip.className).toContain('data-[state=on]:text-primary-text')
  })
})
