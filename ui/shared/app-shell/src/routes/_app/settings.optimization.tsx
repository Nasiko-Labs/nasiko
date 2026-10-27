import { createFileRoute, redirect } from '@tanstack/react-router'

// Settings → Account → Optimization moved to /optimization (plans/feat-optimization-page.md P2, R1D): old links and
// bookmarks land on its Your settings section.
export const Route = createFileRoute('/_app/settings/optimization')({
  beforeLoad: () => {
    throw redirect({ to: '/optimization', hash: 'settings', replace: true })
  },
})
