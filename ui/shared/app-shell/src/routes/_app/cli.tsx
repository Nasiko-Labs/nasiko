import { createFileRoute } from '@tanstack/react-router'
import { CliGuidePage } from '@nasiko/features/cli/CliGuidePage'

export const Route = createFileRoute('/_app/cli')({ component: CliGuidePage })
