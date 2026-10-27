import { createFileRoute, redirect } from '@tanstack/react-router'

// Settings → Security → Chat context (main, PR #27) was a second editor for the same two preferences: they live on
// /optimization's Your settings (plans/feat-optimization-page.md P2), so old links land there (integration 2026-10-05).
export const Route = createFileRoute('/_app/settings/chat-context')({
  beforeLoad: () => {
    throw redirect({ to: '/optimization', hash: 'settings', replace: true })
  },
})
