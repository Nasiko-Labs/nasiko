// Context optimization in a real browser (plans/feat-context-optimization.md eng E5): the keyboard strategy tiles, Save,
// axe, and no sideways scroll on a phone (design review 6A/6B). The /optimization page (plans/feat-optimization-page.md
// eng E6): the chart's Table view by keyboard, the bulk confirm (focus, announcement, axe), the #settings jump, the
// preview waiting for its section, the sticky Save, today's server, and 375 px (R6B).
import { expect, test } from '@playwright/test'
import { expectAccessible } from './axe'

test('Optimization: pick a strategy by keyboard and a budget, then save', async ({ page }) => {
  await page.goto('/optimization#settings')
  await expect(page.getByRole('heading', { level: 1, name: 'Optimization' })).toBeVisible()
  await expect(
    page.getByRole('table', { name: 'What each tier keeps from your last chat' }),
  ).toBeVisible()
  await expectAccessible(page)

  // The tiles are one radio group: arrow keys move between them and Space picks one. (With the radio inside a label,
  // as on Appearance, an arrow moves focus without checking; both pages share that pattern.)
  const pacms = page.getByRole('radio', { name: 'PACMS · Recommended' })
  await expect(pacms).toBeChecked()
  await pacms.focus()
  await page.keyboard.press('ArrowRight')
  await expect(page.getByRole('radio', { name: 'Top-K' })).toBeFocused()
  await page.keyboard.press('Space')
  await expect(page.getByRole('radio', { name: 'Top-K' })).toBeChecked()

  await page.getByRole('radio', { name: 'High' }).click()
  await page.getByRole('button', { name: 'Save changes' }).click()
  await expect(
    page.getByText('Strategy and History budget saved. Applies from your next message.'),
  ).toBeVisible()
  await expect(page.getByRole('button', { name: 'Save changes' })).toBeDisabled()
})

test('Optimization fits a phone @phone', async ({ page }) => {
  // 6A / R6B are specified at 375 px, narrower than the phone project's device.
  await page.setViewportSize({ width: 375, height: 812 })
  await page.goto('/optimization')
  await expect(page.getByRole('heading', { level: 1, name: 'Optimization' })).toBeVisible()
  // R6B: the jump to your settings sits under the lead; the chart draws weeks.
  const lead = page.locator('section', {
    has: page.getByRole('heading', { level: 2, name: /^Saving/ }),
  })
  await expect(lead.getByRole('link', { name: /^Your settings: PACMS · Medium/ })).toBeVisible()
  await expect(lead.getByText(/Click a week to see its biggest senders\./)).toBeVisible()
  await page.locator('#settings').scrollIntoViewIfNeeded()
  await expect(page.getByRole('radio', { name: 'Last-K' })).toBeAttached()
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
  await expectAccessible(page)
})

test('the lead answers first; its Table view picks a day by keyboard', async ({ page }) => {
  await page.goto('/optimization')
  await expect(
    page.getByRole('heading', { level: 2, name: /^Saving ~\d+% of history tokens$/ }),
  ).toBeVisible()
  await expectAccessible(page)
  const table = page.getByRole('button', { name: 'Table', exact: true })
  await table.focus()
  await page.keyboard.press('Enter')
  const pick = page.getByRole('button', { name: /^Show requests/ }).nth(-2)
  await pick.focus()
  await page.keyboard.press('Enter')
  const senders = page.locator('section', {
    has: page.getByRole('heading', { level: 2, name: 'Biggest senders' }),
  })
  await expect(senders.getByText(/ selected$/)).toBeVisible()
  await expect(senders.getByRole('button', { name: /^Clear / })).toBeVisible()
})

test('the bulk turn-on confirm keeps focus, passes axe and announces the end', async ({ page }) => {
  await page.goto('/optimization')
  await page.getByRole('button', { name: /^Turn on for your \d+$/ }).click()
  const dialog = page.getByRole('alertdialog')
  await expect(dialog).toBeVisible()
  await expectAccessible(page)
  for (let i = 0; i < 6; i++) {
    await page.keyboard.press('Tab')
    expect(
      await dialog.evaluate((d) => d.contains(document.activeElement)),
      'focus stays in the dialog',
    ).toBe(true)
  }
  await dialog.getByRole('button', { name: /^Turn on for \d+$/ }).click()
  await expect(
    page.getByText(/^Turned on Token optimization \(Caveman\) for \d+ agents\.$/),
  ).toBeAttached()
  await expect(
    page.getByText(/of your agents have Token optimization \(Caveman\) off/),
  ).toHaveCount(0)
})

test('the header jump focuses Your settings', async ({ page }) => {
  await page.goto('/optimization')
  await page.getByRole('link', { name: /^Your settings: PACMS · Medium/ }).click()
  await expect(page.getByRole('heading', { level: 2, name: 'Your settings' })).toBeFocused()
})

test('the last-chat preview waits until Your settings is near the screen (E4)', async ({
  page,
}) => {
  await page.goto('/optimization')
  await expect(page.getByRole('heading', { level: 2, name: /^Saving/ })).toBeVisible()
  await page.waitForLoadState('networkidle')
  const preview = page.getByRole('table', { name: 'What each tier keeps from your last chat' })
  await expect(preview).toHaveCount(0)
  await page.locator('#settings').scrollIntoViewIfNeeded()
  await expect(preview).toBeVisible()
})

test('while unsaved, Save stays on screen (R2G)', async ({ page }) => {
  await page.goto('/optimization#settings')
  await page.getByRole('radio', { name: 'High' }).click()
  // Bring the top of the card into view, so its footer would be below the fold without the sticky bar.
  await page
    .getByText('2. Context strategy')
    .evaluate((el) => el.scrollIntoView({ block: 'start' }))
  await expect(page.getByText('Unsaved changes')).toBeInViewport()
  await expect(page.getByRole('button', { name: 'Save changes' })).toBeInViewport()
})

test("today's server: every block says what will show there, and passes axe", async ({ page }) => {
  await page.goto('/optimization?mock=optimization-classic')
  await expect(
    page.getByRole('heading', { level: 2, name: 'This server doesn’t report savings yet' }),
  ).toBeVisible()
  await expect(page.getByText('The chats that send the most history will show here.')).toBeVisible()
  await expectAccessible(page)
})

test('a tap on a weekly bar picks its week @phone', async ({ page }) => {
  // R6B + C5 by touch: the kit tracks the mouse only, so the page resolves a tap from where it lands (review: red team).
  await page.setViewportSize({ width: 375, height: 812 })
  await page.goto('/optimization')
  const chart = page.locator('figure div[role="presentation"]').first()
  await expect(chart).toBeVisible()
  const box = await chart.boundingBox()
  if (!box) throw new Error('the chart has no box')
  // The last bar (the latest week) sits just inside the right margin.
  await page.touchscreen.tap(box.x + box.width - 30, box.y + box.height / 2)
  const senders = page.locator('section', {
    has: page.getByRole('heading', { level: 2, name: 'Biggest senders' }),
  })
  await expect(senders.getByText(/^Week of|selected$/).first()).toBeVisible()
  await expect(page).toHaveURL(/slice=/)
})
