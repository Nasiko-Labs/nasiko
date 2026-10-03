import type { ChartPhase } from './chart-phase'

export const LINE_LOADING_PULSE_CYCLE_S = 2.2

/** Idle gap before the loading line pulse restarts (milliseconds). */
export const LINE_LOADING_LOOP_PAUSE_MS = 280

/** Loading label exit on loading → ready (seconds). */
export const LOADING_LABEL_EXIT_S = 0.45

/** Loading label drops this many pixels while exiting. */
export const LOADING_LABEL_EXIT_Y_PX = 30

export const LINE_LOADING_PULSE_EASE = [0.85, 0, 0.15, 1] as const

export type LineLoadingPulseMode = 'loop' | 'exit' | 'enter'

export function resolveLineLoadingPulseMode(phase: ChartPhase): LineLoadingPulseMode | null {
  switch (phase) {
    case 'loading':
      return 'loop'
    case 'exiting':
      return 'exit'
    case 'revealingLoading':
      return 'enter'
    default:
      return null
  }
}
