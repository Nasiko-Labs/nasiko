/**
 * "Open flow" (plans/feat-flows.md F19): one link into a flow from wherever its request shows up (the trace page,
 * a Sessions row, chat's Activity, a workflow run step). A flow's id is its trace id, so the link goes straight to
 * `/flows/<id>`; an id with no flow (a trace that never went through the proxy) lands on the flow page's not-found
 * state, which says why.
 */
import { Link } from '@tanstack/react-router'
import { Network } from 'lucide-react'
import { asButton } from '@nasiko/ui/primitives/button'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'

export function OpenFlowLink({
  flowId,
  latest = false,
  size = 'sm',
  variant = 'outline',
  className,
}: {
  flowId: string
  /** "Open latest flow": the link picks one of several flows (a Sessions row with several traces). */
  latest?: boolean
  size?: 'sm' | 'xs'
  variant?: 'outline' | 'ghost'
  className?: string
}) {
  return (
    <Link
      to="/flows/$flowId"
      params={{ flowId }}
      {...asButton({
        variant: variant,
        size: size,
        className: cn('pointer-coarse:min-h-11', className),
      })}
    >
      <Network className="size-3.5" aria-hidden /> {latest ? copy.openLatestFlow : copy.openFlow}
    </Link>
  )
}
