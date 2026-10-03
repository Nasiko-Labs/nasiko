// The demo script (docs/designs/openruntime-demo-script.md), pinned to its anchor date.
import { expect, test } from '@playwright/test'
import { expectAccessible } from './axe'

const ANCHOR = 'anchor=2026-09-26'

test('TokenOps → Sessions → trace: follow the money', async ({ page }) => {
  await page.goto(`/tokenops?${ANCHOR}`)
  await expect(page.getByTestId('summary-narrative')).toHaveText(
    'In the last 30 days you spent at least $159.28, 31% less than the period before. Code Reviewer drove 33% of it.',
  )
  await expectAccessible(page)

  await page.getByRole('link', { name: /See sessions/ }).click()
  await expect(
    page.getByRole('heading', { level: 1, name: 'Sep 17 · 25 sessions · $18.12' }),
  ).toBeVisible()
  await expectAccessible(page)

  const top = page.getByRole('list', { name: 'Sessions' }).getByRole('link').first()
  await expect(top).toHaveAccessibleName(
    /^Review PR #481 for race conditions · Code Reviewer · \$2\.63 · /,
  )
  await top.click()
  await expect(
    page.getByRole('heading', {
      level: 1,
      name: /^Session · Code Reviewer · Sep 17 16:33 · \$2\.63 · 52 traces/,
    }),
  ).toBeVisible()
  await expect(page.getByLabel('What happened')).toContainText(
    'This trace cost $0.10; retries cost $0.05.',
  )
  await expectAccessible(page)

  await page.goBack()
  await expect(
    page.getByRole('heading', { level: 1, name: 'Sep 17 · 25 sessions · $18.12' }),
  ).toBeVisible()
})

test('Agents: catalog → an agent → its tabs', async ({ page }) => {
  await page.goto(`/agents?${ANCHOR}`)
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
  await expectAccessible(page)
  const first = page.getByRole('main').getByRole('listitem').getByRole('link').first()
  await first.click()
  await expect(page).toHaveURL(/\/agents\/[0-9a-f-]{36}/)
  await expect(page.getByRole('tablist')).toBeVisible()
  await expectAccessible(page)
})

test('Chat: send a message to an agent and read the reply', async ({ page }) => {
  await page.goto(`/chat?${ANCHOR}`)
  await expect(page.getByRole('navigation', { name: 'Chats' })).toBeVisible()
  await expectAccessible(page)
})

test('LLM router: configured routing', async ({ page }) => {
  await page.goto(`/router?${ANCHOR}`)
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
  await expectAccessible(page)
})

test('LLM router: request classification preview', async ({ page }) => {
  await page.goto(`/router?${ANCHOR}&tab=classification`)
  await expect(page.getByRole('heading', { name: 'Request classification' })).toBeVisible()
  await page.getByLabel('Query').fill('Design a rate limiter for a multi-tenant API')
  await page.getByRole('button', { name: 'Test classification' }).click()
  await expect(page.getByRole('region', { name: 'Classification results' })).toBeVisible()
  await expectAccessible(page)
})

test('Harnesses and the status page', async ({ page }) => {
  await page.goto(`/harnesses?${ANCHOR}`)
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
  await expectAccessible(page)
  // `/status`, not `/`: `backend-state` is rendered by the status route
  // (common/src/routes/_app/status.tsx) and exists nowhere else, so this
  // waited 20s on the Overview page for an element that was never coming.
  await page.goto('/status')
  await expect(page.getByTestId('backend-state')).toHaveText('connected')
})

test('the sidebar becomes a sheet on a phone @phone', async ({ page }) => {
  await page.goto(`/tokenops?${ANCHOR}`)
  await page.getByRole('button', { name: 'Open navigation' }).click()
  await expect(page.getByRole('dialog')).toBeVisible()
  await expectAccessible(page)
})
