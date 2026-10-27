import { createFileRoute } from '@tanstack/react-router'
import { OptimizationHubPage } from '@nasiko/features/optimization/OptimizationHubPage'
import { optimizationSearchSchema } from '@nasiko/features/optimization/search'

// Observe → Optimization (plans/feat-optimization-page.md P7): every user. The window keys are TokenOps' (R7A), so a
// link between the two pages keeps the window; `slice` is a chart bar's interval (C5).
export const Route = createFileRoute('/_app/optimization')({
  validateSearch: optimizationSearchSchema,
  component: OptimizationHubPage,
})
