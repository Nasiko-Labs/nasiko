import { z } from 'zod/mini'
import { fallback, flag, isoDate, opt, PRESETS } from '@nasiko/ui/lib/search'

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
  'no-spend',
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
  'assessment-unconfigured',
  'optimization-classic',
  'optimization-no-reports',
  'optimization-down',
  'flows-empty',
  'flows-absent',
  'hub-not-visible',
  'hub-gate-error',
  'hub-deleted',
  'hub-markers-fail',
  'hub-no-savings-row',
  'hub-many-builds',
  'events-absent',
  'ledger-no-trace',
  'home-budgets-absent',
  'overview-absent',
  'overview-quiet',
  'village-unlimited',
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
  // Budget page variants (plans/feat-budget-burn.md); same reason.
  'budgets-empty',
  'budgets-fail',
  'budgets-absent',
  'budgets-rl10-only',
  'budgets-golden',
  // Alert delivery (docs/requirements/alert-delivery.md); same reason.
  'alerts-absent',
  'alerts-nowhere',
  'alerts-failing',
  // Coding-agent onboarding (the card on this page): browser runs from the server, then the desktop app's states.
  'agents-none',
  'agents-importing',
  'agents-sending',
  'agents-rejected',
  'agents-done',
  'agents-absent',
  'desktop-unasked',
  'desktop-running',
  'desktop-failed',
  'desktop-rejected',
  'desktop-sending',
] as const

const id = z.string().check(z.trim(), z.minLength(1), z.maxLength(100))

export const harnessesSearchSchema = z.looseObject({
  preset: fallback(z.enum(PRESETS), '30d'),
  from: opt(isoDate),
  to: opt(isoDate),
  compare: opt(flag),
  harness: opt(id),
  anchor: opt(isoDate),
  mock: opt(z.enum(HARNESS_MOCK_VARIANTS)),
})

export type HarnessesSearch = z.infer<typeof harnessesSearchSchema>
