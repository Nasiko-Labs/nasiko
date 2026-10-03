import { z } from 'zod'
import { flag, isoDate, PRESETS } from '@/lib/search'

/**
 * Harnesses URL state (plan §3): the core's keys. Junk values fall back instead of throwing, so a stale link still
 * opens the page. The schema is loose: an edition layer's page keeps its own keys here (EE: the org level, the
 * metric, the mock persona) and parses them with its own schema (docs/lab-vs-react-migration-review.md §10.4).
 */
/** Mirrors MOCK_VARIANTS in src/mocks/handlers.ts (a test keeps them equal; app code must not import mocks). */
export const HARNESS_MOCK_VARIANTS = [
  'tempo-down',
  'empty',
  'trace-503',
  'trace-500',
  'scan-fail',
  'usage-404',
  // A coded 404 below the landing (EE: a drill to a unit or another developer, served by the EE layer's mocks).
  'drill-404',
  'prev-fail',
  'usage-500',
  'all-unpriced',
  'no-activity',
  'server-down',
  'logout-unavailable',
  // Deploy (plans/feat-deploy.md §9); same reason.
  'deploy-no-rights',
  'builds-absent',
  'deploy-build-fails',
  'deploy-github-unconfigured',
  'deploy-github-disconnected',
  'deploy-github-no-repos',
  'deploy-registry-disabled',
  'deploy-registry-not-running',
  // MCP servers (plans/feat-mcp.md §8); same reason.
  'mcp-empty',
  'mcp-toolkits-fail',
  'mcp-upload-fails',
  // Workflows (plans/feat-workflows.md §8); same reason.
  'workflows-empty',
  'workflows-classic',
  'workflows-no-key',
  'workflows-planner-fails',
  'onboarding-absent',
  'onboarding-done',
  // Chat page variants (v1c DX1); harmless here, listed so the lists stay equal.
  'no-agents',
  'many-chats',
  'many-recorded',
  'probe-500',
  'waiting',
  'pending-fail',
  'pending-flaky',
  // LLM router page variants; same reason.
  'router-empty',
  'router-409',
  'router-custom-down',
  'router-catalog-fail',
  'router-secrets-fail',
  'router-no-secrets',
  'router-repin-fail',
  'router-usage-fail',
  'router-usage-full',
  'router-legacy',
  'router-budgets-empty',
  'router-budgets-fail',
] as const

const id = z.string().trim().min(1).max(100)

export const harnessesSearchSchema = z.looseObject({
  preset: z.enum(PRESETS).default('30d').catch('30d'),
  from: isoDate.optional().catch(undefined),
  to: isoDate.optional().catch(undefined),
  compare: flag.optional().catch(undefined),
  harness: id.optional().catch(undefined),
  anchor: isoDate.optional().catch(undefined),
  mock: z.enum(HARNESS_MOCK_VARIANTS).optional().catch(undefined),
})

export type HarnessesSearch = z.infer<typeof harnessesSearchSchema>
