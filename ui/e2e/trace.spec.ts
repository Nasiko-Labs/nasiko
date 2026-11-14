// NAS-1609: the span panel must fit its column. HR Helpdesk is the mock agent with content capture off, so its
// Prompt & response tab shows the unbreakable OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT, which used to
// widen the panel past its 22rem column and scroll it sideways (the phone's sheet too).
import { expect, test, type Page } from '@playwright/test'

async function openCaptureOffSpan(page: Page) {
  await page.goto('/sessions?anchor=2026-09-26&preset=7d')
  await page
    .getByRole('list', { name: 'Sessions' })
    .getByRole('link', { name: /HR Helpdesk/ })
    .first()
    .click()
  // The root span, as orchestrator.chat was in the report.
  await page.getByRole('treeitem').first().click()
  const panel = page.locator('[data-tour="session-span-panel"]')
  await panel.getByRole('tab', { name: 'Prompt & response' }).click()
  await expect(panel.getByText('Content capture is off').first()).toBeVisible()
  return panel
}

/** Width that the panel or any box around it can scroll sideways, in px (0 when everything fits). */
const sidewaysScroll = (panel: ReturnType<Page['locator']>) =>
  panel.evaluate((el) => {
    let worst = 0
    for (let n: Element | null = el; n; n = n.parentElement)
      worst = Math.max(worst, n.scrollWidth - n.clientWidth)
    return worst
  })

test('the span panel fits its column with content capture off', async ({ page }) => {
  const panel = await openCaptureOffSpan(page)
  expect(await sidewaysScroll(panel)).toBe(0)
})

test('the span sheet fits the screen with content capture off @phone', async ({ page }) => {
  const panel = await openCaptureOffSpan(page)
  expect(await sidewaysScroll(panel)).toBe(0)
})

// Six traces is the most the switcher shows as toggles, a row wider than 1024px less the sidebar: it scrolls on its
// own instead of pushing the page, top bar included, past the window.
test('six traces scroll in their row, not the page', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 900 })
  await page.goto('/sessions/5eed-sess-translator-2026-09-26?anchor=2026-09-26&preset=7d')
  const row = page.getByRole('group', { name: 'Traces in this session' })
  await expect(row.getByRole('button')).toHaveCount(6)
  expect(await row.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true)
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
    ),
  ).toBe(0)
})
