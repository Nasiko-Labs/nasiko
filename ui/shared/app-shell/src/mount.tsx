import { RouterProvider, type AnyRouter } from '@tanstack/react-router'
import { QueryClientProvider } from '@tanstack/react-query'
import { LayoutGroup, LazyMotion, MotionConfig } from 'motion/react'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { EditionProvider } from '@nasiko/app-core/edition/EditionProvider'
import type { Edition } from '@nasiko/app-core/edition/edition'
import { createAppRouter } from '@nasiko/app-shell/router'
import { watchOtherTabs } from '@nasiko/app-shell/shell/sessionSync'
import { initTheme } from '@nasiko/app-core/shell/theme'
import { env } from '@nasiko/app-core/env'
import { installStaleBuildDetection } from '@nasiko/app-core/staleBuild'
import { createQueryClient } from '@nasiko/app-core/queryClient'

const loadMotionFeatures = () => import('@nasiko/ui/lib/motionFeatures').then((m) => m.default)

/**
 * Boot an edition (docs/lab-vs-react-migration-review.md §10.2): each edition's `src/main.tsx` calls this with the
 * edition its `src/edition.ts` declares. The route tree is the edition's own (`@edition/routeTree.gen`).
 */
export async function mount(edition: Edition) {
  // A holder, since the query client is built before the router it redirects with.
  const routerRef: { current?: AnyRouter } = {}
  const queryClient = createQueryClient(() => routerRef.current)
  const router = createAppRouter({ queryClient })
  routerRef.current = router as unknown as AnyRouter
  watchOtherTabs({ queryClient, router: routerRef.current })
  installStaleBuildDetection()

  initTheme()
  const mocking = env.mode === 'mock' || env.partialMocks.length > 0
  // eslint-disable-next-line no-console -- documented boot log (CLAUDE.md: startup logs the mode)
  console.info(
    `[ui-lab] mode=${env.mode}${env.partialMocks.length ? ` mocked=${env.partialMocks.join(',')}` : ''} · API via Vite proxy (NASIKO_API_URL)`,
  )
  if (mocking) {
    try {
      // Dynamic import: MSW never ships in a live-only bundle (A22).
      const { startMocks } = await import('@nasiko/mocks/browser')
      await startMocks(env)
    } catch (err) {
      console.error(
        '[ui-lab] Mock mode, but the MSW worker failed to start. Run `npx msw init public` and reload.',
        err,
      )
      // Built with DOM calls and Tailwind classes, not injected HTML with an inline style: a CSP without
      // 'unsafe-inline' blocks both (plan §8 Phase 9).
      const banner = document.createElement('div')
      banner.setAttribute('role', 'alert')
      banner.className = 'bg-warning/15 px-4 py-2 text-sm text-foreground'
      const cmd = document.createElement('code')
      cmd.textContent = 'npx msw init public'
      banner.append('Mock mode, but the MSW worker failed to start. Run ', cmd, ' and reload.')
      document.body.prepend(banner)
    }
  }
  const root = document.getElementById('root')
  if (!root) throw new Error('index.html has no #root element')
  createRoot(root).render(
    <StrictMode>
      <EditionProvider edition={edition}>
        <QueryClientProvider client={queryClient}>
          {/* No TooltipProvider here: it pulls Base UI's tooltip and floating-ui code into the shell, so the app layout
              (routes/_app.tsx, its own chunk) provides it around every page that has tooltips. */}
          <MotionConfig reducedMotion="user">
            {/* domMax: shared-layout (layoutId) morphs between pages need the layout features. It loads after
              first paint (its own chunk, off the shell budget); `m` components render at rest until then. */}
            <LazyMotion features={loadMotionFeatures}>
              <LayoutGroup>
                <RouterProvider router={router} />
              </LayoutGroup>
            </LazyMotion>
          </MotionConfig>
        </QueryClientProvider>
      </EditionProvider>
    </StrictMode>,
  )
}
