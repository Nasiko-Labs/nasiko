'use client'

import { createContext, useContext } from 'react'

interface ChartLegendHoverContextValue {
  hoveredIndex: number | null
  setHoveredIndex: (index: number | null) => void
}

const ChartLegendHoverContext = createContext<ChartLegendHoverContextValue | null>(null)

export function useChartLegendHover(): ChartLegendHoverContextValue {
  const context = useContext(ChartLegendHoverContext)
  return (
    context ?? {
      hoveredIndex: null,
      setHoveredIndex: () => {
        /* noop outside ChartLegendHoverProvider */
      },
    }
  )
}
