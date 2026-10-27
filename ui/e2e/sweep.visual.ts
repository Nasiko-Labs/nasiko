// The visual sweep (e2e/sweep.config.ts has how to run it). Per edition: the route list itself, every page in light
// and dark, the first popup each page offers, and how navigating between pages moves the content.
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { expect, test, type Page } from '@playwright/test'
import { SWEEP_ANCHOR } from './sweep.config.ts'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')

/** The edition's pages as its router knows them (`/agents`, `/agents/$agentId`), from the generated route tree. */
function routesOf(dir: string): string[] {
  const tree = readFileSync(join(ROOT, dir, 'src/routeTree.gen.ts'), 'utf8')
  const block = /interface FileRoutesByFullPath \{([^}]*)\}/.exec(tree)?.[1] ?? ''
  const paths = [...block.matchAll(/'([^']+)':/g)].map((m) => (m[1] ?? '').replace(/(.)\/$/, '$1'))
  return [...new Set(paths)].sort()
}

/**
 * The seed and the app read "now" as the anchor, but the app's clock then runs on (shared/mocks/src/anchor.ts), so a
 * figure accrued over time (this month's spend, a build's progress) would differ by the seconds a run takes. Frozen.
 */
const freezeClock = (page: Page) =>
  page.clock.setFixedTime(new Date(`${SWEEP_ANCHOR}T15:00:00.000Z`))

const fileName = (route: string) => route.replace(/^\//, '').replace(/[/$]+/g, '-') || 'index'

/** Loaded and idle: no request in flight, no loader or busy region left, fonts in. */
async function settle(page: Page) {
  // Capped: a page that streams (a build's log) never goes idle, and is shot as it stands.
  await page.waitForLoadState('networkidle', { timeout: 10_000 }).catch(() => undefined)
  await expect.soft(page.locator('[aria-busy="true"]')).toHaveCount(0)
  await page.evaluate(() => document.fonts.ready)
}

test('every page looks as it did', async ({ page }, info) => {
  await freezeClock(page)
  const routes = routesOf(String(info.project.metadata.dir))
  // A page added or removed shows here first; the screenshots below can't tell a missing page from a renamed one.
  expect(`${routes.join('\n')}\n`).toMatchSnapshot('routes.txt')

  const fixed = routes.filter((r) => !r.includes('$'))
  const withParams = routes.filter((r) => r.includes('$'))
  // A page with a parameter is reached through the first link to it on a page without one.
  const linked = new Map<string, string>()
  const patterns = withParams.map((r) => ({
    route: r,
    re: new RegExp(`^${r.replace(/\$[^/]+/g, '[^/]+')}$`),
  }))

  // Menus and popovers: the primitives that a swap of the component library redraws. Not a Select's trigger
  // (`role="combobox"`): Radix's carried no `aria-haspopup`, so the baseline never opened one, and Base UI's says
  // `listbox`; counting them would change which popup a page is shot with.
  const popup = page.locator('main button[aria-haspopup]:not([role="combobox"]):visible').first()

  // Loaded and shot again until it matches: a page that lands a few frames early or late (a chart drawing in, a panel
  // still opening) matches on a later try, a page that really changed never does.
  const unmatched: string[] = []
  async function shot(url: string, name: string, open = false) {
    await expect(async () => {
      await page.goto(url)
      await settle(page)
      // A full-page shot doesn't scroll, so content that renders on coming into view (a chart below the fold)
      // would be caught empty: bring all of it into view first.
      await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight))
      await settle(page)
      await page.evaluate(() => window.scrollTo(0, 0))
      if (open) {
        await popup.click()
        await settle(page)
      }
      await expect(page).toHaveScreenshot(name, { fullPage: !open })
    })
      // Room for one load of a page that never goes idle (two capped waits) and a few more tries.
      .toPass({ intervals: [500], timeout: 45_000 })
      .catch(() => unmatched.push(name))
  }

  async function shoot(url: string, name: string) {
    for (const colorScheme of ['light', 'dark'] as const) {
      await page.emulateMedia({ colorScheme })
      await shot(url, `${name}.${colorScheme}.png`)
    }
    if ((await popup.count()) > 0) await shot(url, `${name}.popup.png`, true)
  }

  for (const route of fixed) {
    await test.step(route, async () => {
      await shoot(route, fileName(route))
      const hrefs = await page
        .locator('a[href^="/"]')
        .evaluateAll((as) => as.map((a) => new URL((a as HTMLAnchorElement).href).pathname))
      for (const href of hrefs)
        for (const { route: r, re } of patterns)
          if (!linked.has(r) && !fixed.includes(href) && re.test(href)) linked.set(r, href)
    })
  }
  for (const route of withParams) {
    const url = linked.get(route)
    if (!url) {
      info.annotations.push({ type: 'not reached', description: `${route}: no page links to it` })
      continue
    }
    await test.step(`${route} (${url})`, () => shoot(url, fileName(route)))
  }
  // Each one's expected, actual and diff images are in test-results/.
  expect(unmatched, 'screenshots that never matched the baseline').toEqual([])
})

test('navigating moves the content as it did', async ({ page }) => {
  // Motion on: this is the one check of how pages enter, so the sweep's reduced motion is lifted.
  await page.emulateMedia({ reducedMotion: 'no-preference' })
  await freezeClock(page)
  /** Home, with the starting guide (opened on a first visit) closed: its overlay would take the clicks. */
  async function home() {
    await page.goto('/')
    await settle(page)
    if ((await page.getByRole('dialog').count()) > 0) await page.keyboard.press('Escape')
  }
  await home()
  const hrefs = await page
    .locator('nav a[href^="/"]:visible')
    .evaluateAll((as) => [...new Set(as.map((a) => a.getAttribute('href') ?? '/'))])
  // `/` may have redirected (to Home or a sign-in): its own link would not navigate.
  const here = new URL(page.url()).pathname
  const seen: Record<string, { faded: boolean; slid: boolean; rose: boolean; blank: boolean }> = {}
  for (const href of hrefs.filter((h) => h !== '/' && h !== here).slice(0, 4)) {
    // From Home each time: a page may swap the sidebar for its own panel (Chat), hiding the next link.
    await home()
    // Every frame for three seconds: each element near the top of <main> (its own opacity and translation, which
    // animations change and layout does not), and whether <main> has any text at all.
    await page.evaluate(() => {
      type Range = [number, number]
      const w = window as unknown as {
        __motion: { o: Range; x: Range; y: Range }[]
        __blank: boolean
        __raf: number
      }
      const ranges = new Map<Element, { o: Range; x: Range; y: Range }>()
      const widen = (r: Range, v: number) => {
        r[0] = Math.min(r[0], v)
        r[1] = Math.max(r[1], v)
      }
      w.__blank = false
      let frames = 0
      const tick = () => {
        const main = document.querySelector('main')
        if (main) {
          if (!main.textContent?.trim()) w.__blank = true
          const near = main.querySelectorAll(
            ':scope > *, :scope > * > *, :scope > * > * > *, :scope > * > * > * > *',
          )
          for (const el of [main, ...near]) {
            const s = getComputedStyle(el)
            const m = new DOMMatrixReadOnly(s.transform === 'none' ? undefined : s.transform)
            const [tx = '0', ty = '0'] = s.translate === 'none' ? [] : s.translate.split(' ')
            const x = m.m41 + parseFloat(tx)
            const y = m.m42 + parseFloat(ty)
            const r = ranges.get(el)
            if (r) {
              widen(r.o, +s.opacity)
              widen(r.x, x)
              widen(r.y, y)
            } else ranges.set(el, { o: [+s.opacity, +s.opacity], x: [x, x], y: [y, y] })
          }
        }
        w.__motion = [...ranges.values()]
        if (++frames < 180) w.__raf = requestAnimationFrame(tick)
      }
      tick()
    })
    const from = page.url()
    await page.locator(`nav a[href="${href}"]:visible`).first().click()
    // Any change, not the link's own path: a link may land on a sub-page (Settings opens its first section).
    await page.waitForURL((u) => u.href !== from)
    await settle(page)
    await page.waitForTimeout(500)
    const { motion, blank } = await page.evaluate(() => {
      const w = window as unknown as {
        __motion: Record<'o' | 'x' | 'y', [number, number]>[]
        __blank: boolean
        __raf: number
      }
      cancelAnimationFrame(w.__raf)
      return { motion: w.__motion, blank: w.__blank }
    })
    const varied = (key: 'o' | 'x' | 'y', by: number) =>
      motion.some((r) => r[key][1] - r[key][0] > by)
    // Booleans, not numbers: frame timing differs between runs, whether the page fades or slides does not.
    seen[href] = { faded: varied('o', 0.1), slid: varied('x', 8), rose: varied('y', 4), blank }
  }
  expect(`${JSON.stringify(seen, null, 2)}\n`).toMatchSnapshot('navigation.json')
})
