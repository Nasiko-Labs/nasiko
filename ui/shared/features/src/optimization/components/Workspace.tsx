/**
 * /optimization's Workspace card, beside Your settings (plans/feat-optimization-page.md R1B, R3A, R5B; §7 Workspace row): a quiet row for
 * superusers with what the workspace sets, each with its edit link. Members don't see it (they already see their
 * tier's figure in Your settings). The core's row is the tiers (labelled defaults on a server that doesn't report
 * them, CX-T1); a layer adds its rows through the `optimizationWorkspace` slot (EE: Organization policy).
 */
import { useQuery } from '@tanstack/react-query'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { Card } from '@nasiko/ui/primitives/card'
import { meQuery } from '@nasiko/data-access/auth'
import { tiersQuery } from '../api'
import { copy } from '../copy'
import { SERVER_DEFAULT_TIERS } from '../types'
import { WorkspaceRow } from './WorkspaceRow'

export function Workspace() {
  const me = useQuery(meQuery)
  const tiers = useQuery(tiersQuery)
  const { optimizationWorkspace: Extra } = useSlots()
  if (me.data?.is_superuser !== true) return null
  const values = tiers.data ?? SERVER_DEFAULT_TIERS
  return (
    <Card className="gap-3 p-4" render={<section aria-labelledby="optimization-workspace-h" />}>
      <h2 id="optimization-workspace-h" className="text-sm font-semibold">
        {copy.workspace.title}
      </h2>
      <ul className="m-0 flex list-none flex-col p-0">
        <WorkspaceRow
          label={tiers.data === null ? copy.workspace.defaults : copy.workspace.tiers}
          value={
            tiers.isPending
              ? undefined
              : copy.workspace.tierValues(
                  values.pacms_budget.low,
                  values.pacms_budget.medium,
                  values.pacms_budget.high,
                )
          }
          failed={tiers.isError && !tiers.data ? copy.workspace.tiersFailed : undefined}
          onRetry={() => void tiers.refetch()}
          to="/settings/optimization-tiers"
          action={copy.workspace.editTiers}
        />
        {Extra ? <Extra /> : null}
      </ul>
    </Card>
  )
}
