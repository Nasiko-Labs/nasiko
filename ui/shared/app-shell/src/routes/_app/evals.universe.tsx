import { createFileRoute } from '@tanstack/react-router'
import { EvalUniversePage } from '@nasiko/features/evals/universe/EvalUniversePage'

/** Eval Universe: every code review task/attempt as a navigable 3D scene
 * (`oss/docs/proposals/evals/CODE_REVIEW_EVALS.md`). Reached from Code review evals' own header
 * link, not its own sidebar entry — see `nav.ts`'s `/evals` item
 * (`also: ['/evals/code-review', '/evals/universe']`). */
export const Route = createFileRoute('/_app/evals/universe')({
  component: EvalUniversePage,
})
