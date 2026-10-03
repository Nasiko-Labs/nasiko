/**
 * The `?mock=` router page variants (plan §6), kept apart from src/mocks so the app bundle never imports mock code
 * (bundle-boundary test). Must match `ROUTER_PAGE_VARIANTS` in src/mocks/handlers.ts (logic.test.ts checks it) and
 * the list in docs/router.md.
 */
export const ROUTER_VARIANT_KEYS = [
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
  'router-classifier-regex',
  'router-classifier-unconfigured',
  'router-classifier-fail',
  'router-classifier-laya',
  'router-classifier-laya-missing',
] as const
