import { createFileRoute } from '@tanstack/react-router'
import { CodeReviewPage } from '@nasiko/features/evals/code-review/CodeReviewPage'

/** Code review evals: import a pull request, review it with chosen models, compare results
 * (`oss/docs/proposals/evals/CODE_REVIEW_EVALS.md`). Reached from Evals' own header link, not its
 * own sidebar entry — see `nav.ts`'s `/evals` item (`also: ['/evals/code-review']`). */
export const Route = createFileRoute('/_app/evals/code-review')({
  component: CodeReviewPage,
})
