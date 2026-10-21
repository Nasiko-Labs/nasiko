// Ported from nasiko-ui-lab@1b99bdda common/src/features/optimization/components/Workspace.tsx. Local edits: `"use
// client"`; `meQuery`/`tiersQuery` are `meQueryOptions(scope)`/`tiersQuery(scope)`; Prettier formatting.
// The redesign of TokenOps, Optimization, Overview and Harnesses (legacy ui/ on development, 3aac84f66: 8bf09bf29),
// merged into this port.
// `Card asChild` -> `render`.
/**
 * /optimization's Workspace card, beside Your settings (plans/feat-optimization-page.md R1B, R3A, R5B; §7 Workspace row): a quiet row for
 * superusers with what the workspace sets, each with its edit link. Members don't see it (they already see their
 * tier's figure in Your settings). The core's row is the tiers (labelled defaults on a server that doesn't report
 * them, CX-T1); a layer adds its rows through the `optimizationWorkspace` slot (EE: Organization policy).
 */
"use client";

import { useQuery } from "@tanstack/react-query";
import { useSlots } from "@nasiko/app-core/edition/edition-context";
import { Card } from "@nasiko/ui/primitives";
import { meQueryOptions, useProductScope } from "@nasiko/data-access/product";
import { tiersQuery } from "../api";
import { copy } from "../copy";
import { SERVER_DEFAULT_TIERS } from "../types";
import { WorkspaceRow } from "./WorkspaceRow";

export function Workspace() {
  const scope = useProductScope();
  const me = useQuery(meQueryOptions(scope));
  const tiers = useQuery(tiersQuery(scope));
  const { optimizationWorkspace: Extra } = useSlots();
  if (me.data?.is_superuser !== true) return null;
  const values = tiers.data ?? SERVER_DEFAULT_TIERS;
  return (
    <Card render={<section aria-labelledby="optimization-workspace-h" />} className="gap-3 p-4">
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
  );
}
