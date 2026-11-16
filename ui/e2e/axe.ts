import AxeBuilder from '@axe-core/playwright'
import { expect, type Page } from '@playwright/test'

/** WCAG 2.1 A/AA: no serious or critical violation on the page as it stands. */
export async function expectAccessible(page: Page) {
  // Settled data only: stale numbers are dimmed on purpose while a refresh is in flight.
  await page.waitForLoadState('networkidle')
  const { violations } = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
    .analyze()
  const blocking = violations
    .filter((v) => v.impact === 'serious' || v.impact === 'critical')
    .map(
      (v) =>
        `${v.id} (${v.impact}): ${v.nodes
          .slice(0, 3)
          .map(
            (n) =>
              `${n.target.join(' ')} [${n.html.slice(0, 120)}] ${(n.failureSummary ?? '').replace(/\s+/g, ' ')}`,
          )
          .join(' | ')}`,
    )
  expect(blocking, 'axe: serious/critical violations').toEqual([])
}
