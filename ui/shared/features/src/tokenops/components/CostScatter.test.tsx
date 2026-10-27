import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { CostScatter, type ScatterPoint } from './CostScatter'

const PLOT = { width: 400, height: 300 }
const TIP_HEIGHT = 96

vi.mock('@visx/responsive', () => ({
  ParentSize: ({ children }: { children: (size: typeof PLOT) => ReactNode }) => children(PLOT),
}))

const point = (id: string, x: number, y: number): ScatterPoint => ({
  id,
  name: `agent ${id}`,
  x,
  y,
  z: 10,
  hours: null,
})

const POINTS = [point('low', 0.001, 10), point('mid', 0.01, 500), point('top', 0.1, 1000)]

let restore: () => void

beforeEach(() => {
  const original = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight')
  Object.defineProperty(HTMLElement.prototype, 'offsetHeight', {
    configurable: true,
    get() {
      return (this as HTMLElement).classList.contains('bg-popover') ? TIP_HEIGHT : 0
    },
  })
  restore = () => {
    if (original) Object.defineProperty(HTMLElement.prototype, 'offsetHeight', original)
  }
})

afterEach(() => restore())

async function hoverPoint(index: number) {
  const { container } = render(<CostScatter points={POINTS} onSelect={() => undefined} />)
  const circles = container.querySelectorAll('circle')
  await userEvent.hover(circles[index] as Element)
  const tip = (await screen.findByText(POINTS[index]?.name ?? '')).parentElement as HTMLElement
  return { tip, cy: Number(circles[index]?.getAttribute('cy')) }
}

describe('CostScatter tooltip', () => {
  it('keeps the tooltip inside the plot for a point at the top edge', async () => {
    const { tip } = await hoverPoint(2)
    const top = parseFloat(tip.style.top)
    expect(top).toBeGreaterThanOrEqual(0)
    expect(top + TIP_HEIGHT).toBeLessThanOrEqual(PLOT.height)
    expect(tip.style.transform).not.toContain('-50%')
  })

  it('keeps the tooltip inside the plot for a point at the bottom edge', async () => {
    const { tip } = await hoverPoint(0)
    const top = parseFloat(tip.style.top)
    expect(top).toBeGreaterThanOrEqual(0)
    expect(top + TIP_HEIGHT).toBeLessThanOrEqual(PLOT.height)
  })

  it('centres the tooltip on a point with room above and below', async () => {
    const { tip, cy } = await hoverPoint(1)
    expect(parseFloat(tip.style.top)).toBeCloseTo(8 + cy - TIP_HEIGHT / 2, 5)
  })
})
