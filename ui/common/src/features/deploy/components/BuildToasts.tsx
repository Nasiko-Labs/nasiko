/**
 * The app's one `Toaster` (eng review) and the follower's notifier (plans/feat-deploy.md §5 Background follow): a build
 * that finishes away from its page says so once, with one action. Sonner's own region announces it; pages keep theirs.
 */
import { useNavigate } from '@tanstack/react-router'
import { useEffect } from 'react'
import { toast } from 'sonner'
import { Toaster } from '@/components/ui/sonner'
import { copy } from '../copy'
import { setNotifier } from '../follower'

export function BuildToasts() {
  const navigate = useNavigate()
  useEffect(
    () =>
      setNotifier((n) => {
        if (n.kind === 'running') {
          toast.success(copy.build.running(n.name), {
            action: {
              label: copy.build.chat,
              onClick: () =>
                void navigate({ to: '/chat', search: { agent: n.agentId ?? n.name } as never }),
            },
          })
        } else if (n.kind === 'failed') {
          toast.error(copy.toast.failed(n.name), {
            action: {
              label: copy.toast.seeWhy,
              onClick: () =>
                void navigate({ to: '/builds/$buildId', params: { buildId: n.buildId } }),
            },
          })
        } else {
          toast.warning(copy.toast.notRunning(n.name), {
            action: {
              label: copy.build.openAgent,
              onClick: () =>
                void navigate({
                  to: '/agents/$agentId',
                  params: { agentId: n.agentId },
                  search: {},
                }),
            },
          })
        }
      }),
    [navigate],
  )
  return <Toaster position="bottom-right" />
}
