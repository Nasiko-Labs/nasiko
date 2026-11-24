/**
 * The Evals flow as stepper steps. The first is the dataset's source: the harness's sessions, or, when it has none, the
 * pre-defined SWE benchmarks.
 */
import { Database, ListChecks, Scale, Trophy } from 'lucide-react'
import type { StepperStep } from '@nasiko/ui/composites/stepper'
import { copy } from './copy'

export const STEP = { source: 0, dataset: 1, compare: 2 } as const
export type Step = (typeof STEP)[keyof typeof STEP]

export type Source = 'sessions' | 'benchmarks'

/** The steps, each with what was picked there so far (shown under its title). */
export const evalSteps = (
  source: Source,
  picked: { source?: string | undefined; dataset?: string | undefined } = {},
): StepperStep[] => [
  source === 'sessions'
    ? { title: copy.steps.sessions, icon: ListChecks, detail: picked.source }
    : { title: copy.steps.benchmarks, icon: Trophy, detail: picked.source },
  { title: copy.steps.dataset, icon: Database, detail: picked.dataset },
  { title: copy.steps.compare, icon: Scale },
]
