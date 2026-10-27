import { createFileRoute } from '@tanstack/react-router'
import { AssessmentPage } from '@nasiko/features/assessment/AssessmentPage'

// The free TokenOps assessment (the sidebar card that replaced the waitlist's): every user.
export const Route = createFileRoute('/_app/assessment')({ component: AssessmentPage })
