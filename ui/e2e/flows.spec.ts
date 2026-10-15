// Flows in a real browser (plans/feat-flows.md, eng review test plan): the list (axe, Kind filter, into a flow), the
// flow page's swimlane by keyboard (one tab stop, arrows, Enter selects, Esc clears) with axe, "← Flows" back to the
// list as it was left, and no sideways scroll on a phone.
import { expect, test } from '@playwright/test'
import { expectAccessible } from './axe'

const SHOWCASE = '5eed0014000000000000000000000001'

test('Flows: filter by kind, open a flow, come back as it was', async ({ page }) => {
  await page.goto('/flows')
  await expect(page.getByRole('heading', { level: 1, name: 'Flows' })).toBeVisible()
  await expect(page.getByRole('table', { name: 'Flows' })).toBeVisible()
  await expectAccessible(page)
  await page.getByRole('radio', { name: 'Orchestrated' }).click()
  await expect(page).toHaveURL(/kind=orchestrated/)
  await page
    .getByRole('link', { name: 'Compare 2026 pricing for three vendors and draft a summary' })
    .click()
  await expect(page.getByRole('heading', { level: 1, name: /Compare 2026 pricing/ })).toBeVisible()
  await page.getByRole('link', { name: 'Back to Flows' }).click()
  await expect(page).toHaveURL(/\/flows\?.*kind=orchestrated/)
  await expect(page.getByRole('radio', { name: 'Orchestrated' })).toHaveAttribute(
    'aria-checked',
    'true',
  )
})

test('the swimlane by keyboard: one tab stop, arrows, Enter selects, Esc clears', async ({
  page,
}) => {
  await page.goto(`/flows/${SHOWCASE}`)
  const chart = page.getByRole('group', { name: /^Timeline of 6 agent calls/ })
  await expect(chart).toBeVisible()
  await expect(page.getByText('Recorded steps + calls from traces')).toBeVisible()
  await expectAccessible(page)

  const bars = chart.getByRole('button')
  await expect(bars.and(page.locator('[tabindex="0"]'))).toHaveCount(1)
  await bars.and(page.locator('[tabindex="0"]')).focus()
  await page.keyboard.press('ArrowRight')
  await expect(page.locator(':focus')).toHaveAttribute('aria-label', /^seed-code-reviewer/)
  await page.keyboard.press('End')
  await expect(page.locator(':focus')).toHaveAttribute('aria-label', /Draft a one-page comparison/)
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(/step=step%3A/)
  await expect(
    page.getByText('Drafted a comparison table and a three-line recommendation.'),
  ).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page).not.toHaveURL(/step=/)

  await page.getByRole('radio', { name: 'Table' }).click()
  await expect(page.getByRole('table', { name: 'Calls and their timing' })).toBeVisible()
  await expectAccessible(page)
})

test('phones: the flow opens on its Table, the timeline scrolls inside its panel; the list uses Selects @phone', async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 812 })
  const pageOverflow = () =>
    page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)

  await page.goto(`/flows/${SHOWCASE}`)
  await expect(page.getByRole('table', { name: 'Calls and their timing' })).toBeVisible()
  expect(await pageOverflow()).toBeLessThanOrEqual(0)
  await expectAccessible(page)
  await page.getByRole('radio', { name: 'Timeline' }).click()
  await expect(page.getByRole('group', { name: /^Timeline of/ })).toBeVisible()
  expect(await pageOverflow()).toBeLessThanOrEqual(0)
  await expectAccessible(page)

  await page.goto('/flows')
  await expect(page.getByRole('combobox', { name: 'Kind' })).toBeVisible()
  await expect(page.getByRole('radio', { name: 'Orchestrated' })).toBeHidden()
  expect(await pageOverflow()).toBeLessThanOrEqual(0)
  await expectAccessible(page)
})
