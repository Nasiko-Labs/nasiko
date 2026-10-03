import { Badge } from '@/components/ui/badge'
import { STATUS_LABEL } from '../copy'
import type { ServerStatus } from '../logic'

const TONE = {
  active: 'success',
  inactive: 'muted',
  building: 'info',
  failed: 'destructive',
} as const satisfies Record<ServerStatus, string>

/** Text plus colour, never a colour alone (as the agent badge). */
export function ServerStatusBadge({ status }: { status: ServerStatus }) {
  return (
    <Badge variant={TONE[status]} data-status={status}>
      {STATUS_LABEL[status]}
    </Badge>
  )
}
