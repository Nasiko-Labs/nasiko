import { createFileRoute } from '@tanstack/react-router'
import { EvalsPage } from '@nasiko/features/evals/EvalsPage'

/** Your connected coding harnesses, to build evals from (docs/superpowers/specs/2026-11-23-evals-design.md). */
export const Route = createFileRoute('/_app/evals/')({ component: EvalsPage })
