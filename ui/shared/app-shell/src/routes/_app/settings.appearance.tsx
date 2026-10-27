import { createFileRoute, redirect } from '@tanstack/react-router'

// Account settings moved out of the workspace's Settings to /account (account menu → Settings): old links land there.
export const Route = createFileRoute('/_app/settings/appearance')({
  beforeLoad: () => {
    throw redirect({ to: '/account/appearance', replace: true })
  },
})
