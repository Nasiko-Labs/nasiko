import { createFileRoute, redirect } from '@tanstack/react-router'

// Account settings moved out of the workspace's Settings to /account (account menu → Settings): old links land there.
export const Route = createFileRoute('/_app/settings/password')({
  beforeLoad: () => {
    throw redirect({ to: '/account/password', replace: true })
  },
})
