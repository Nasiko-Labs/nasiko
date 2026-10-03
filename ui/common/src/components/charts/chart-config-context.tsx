'use client'

import { createContext, useContext } from 'react'

export interface SpringConfig {
  stiffness: number
  damping: number
}

export interface ChartConfigValue {
  /** Crosshair indicator, tooltip dot, date pill. */
  tooltipSpring: SpringConfig
  /** Floating tooltip panel. */
  tooltipBoxSpring: SpringConfig
  /** Line/area hover-highlight band (x + width). */
  highlightSpring: SpringConfig
}

const DEFAULT_CHART_CONFIG: ChartConfigValue = {
  tooltipSpring: { stiffness: 300, damping: 30 },
  tooltipBoxSpring: { stiffness: 100, damping: 20 },
  highlightSpring: { stiffness: 180, damping: 28 },
}

const ChartConfigContext = createContext<ChartConfigValue | null>(null)

export function useChartConfig(): ChartConfigValue {
  return useContext(ChartConfigContext) ?? DEFAULT_CHART_CONFIG
}

const DEFAULT_TOOLTIP_BOX_DAMPING = DEFAULT_CHART_CONFIG.tooltipBoxSpring.damping

/** Maps a damping slider to the floating tooltip panel follow spring. `0` = instant. */
export function resolveTooltipBoxMotion(damping?: number): {
  animate: boolean
  springConfig: SpringConfig
} {
  if (damping === 0) {
    return {
      animate: false,
      springConfig: DEFAULT_CHART_CONFIG.tooltipBoxSpring,
    }
  }

  const effectiveDamping = damping ?? DEFAULT_TOOLTIP_BOX_DAMPING
  let stiffness = DEFAULT_CHART_CONFIG.tooltipBoxSpring.stiffness

  if (effectiveDamping < DEFAULT_TOOLTIP_BOX_DAMPING) {
    const t = (DEFAULT_TOOLTIP_BOX_DAMPING - effectiveDamping) / DEFAULT_TOOLTIP_BOX_DAMPING
    stiffness += t * 400
  } else if (effectiveDamping > DEFAULT_TOOLTIP_BOX_DAMPING) {
    const t = (effectiveDamping - DEFAULT_TOOLTIP_BOX_DAMPING) / (100 - DEFAULT_TOOLTIP_BOX_DAMPING)
    stiffness -= t * 85
  }

  return {
    animate: true,
    springConfig: {
      stiffness: Math.max(12, Math.round(stiffness)),
      damping: effectiveDamping,
    },
  }
}
