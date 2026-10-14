// Ported from nasiko-ui-lab@1b99bdda common/src/features/mcp/components/AgentMcpTab.tsx. Local edits: `meQuery` is
// `meQueryOptions(useProductScope())`; the lab's `EmptyState` is `LabEmptyState`; `Button asChild` + `Link` ->
// `ButtonLink` (agents/components/ButtonLink.tsx, a navigation-port `Link` anchor with the button classes); Prettier
// formatting.
"use client";

/**
 * The agent detail's MCP tab (plans/feat-mcp.md §5.2, legacy agent page "Configure → MCP"): the MCP servers this agent
 * may use, one `ConnectorRules` each. The server lists only servers the caller has connected, plus no-auth ones; an
 * upload that is still building or failed is one of those, so it shows greyed with its status (`UnavailableRow`).
 * Its build state comes from the catalog list (polled while a build runs), so a finished build turns into a row.
 */
import { useQuery } from "@tanstack/react-query";
import { RotateCw } from "lucide-react";
import { Button } from "@nasiko/ui/primitives";
import { PageLoader } from "@nasiko/ui/composites";
import { LabEmptyState as EmptyState, StateCard } from "@nasiko/ui/composites";
import { Section } from "../../agents/components/bits";
import { ButtonLink } from "../../agents/components/ButtonLink";
import { meQueryOptions, useProductScope } from "@nasiko/data-access/product";
import { useAgentConnectors, useConnectors } from "../api";
import { copy, reason } from "../copy";
import { labelOf, unusable } from "../logic";
import { ConnectorRules, UnavailableRow } from "./ConnectorRules";

export function AgentMcpTab({ agentId, harness = false }: { agentId: string; harness?: boolean }) {
  const list = useAgentConnectors(agentId, true);
  // A failed catalog read just means no row is greyed: the agent list itself still works.
  const servers = useConnectors();
  const me = useQuery(meQueryOptions(useProductScope())).data;
  const notReady = unusable(servers.data);
  const owned = new Set(servers.data?.created_by_you.map((c) => c.connector_id));
  return (
    <Section title={copy.agentMcpTitle}>
      <p className="text-sm text-muted-foreground">
        {harness ? copy.agentMcpSubHarness : copy.agentMcpSub}
      </p>
      {/* eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- the lab's guard, kept as written */}
      {list.isPending || (servers.isPending && !servers.isError) ? (
        <PageLoader label={copy.loadingCatalog} inline className="min-h-64" />
      ) : list.isError ? (
        <StateCard
          tone="error"
          title={copy.agentFailed}
          fix={reason(list.error)}
          action={
            <Button size="sm" variant="outline" onClick={() => void list.refetch()}>
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          }
        />
      ) : !list.data.length ? (
        <EmptyState
          title={copy.noAgentServers}
          action={
            <ButtonLink href="/mcp" size="sm" variant="outline">
              {copy.openCatalog}
            </ButtonLink>
          }
        >
          {copy.noAgentServersFix}
        </EmptyState>
      ) : (
        <ul className="space-y-2">
          {list.data.map((c) => {
            const target = {
              connectorId: c.connector_id,
              label: labelOf(c),
              logoUrl: c.logo_url,
              enabled: c.enabled,
            };
            const status = notReady.get(c.connector_id);
            return (
              <li key={c.connector_id}>
                {status ? (
                  <UnavailableRow
                    target={target}
                    status={status}
                    logs={owned.has(c.connector_id) || !!me?.is_superuser}
                  />
                ) : (
                  <ConnectorRules agentId={agentId} target={target} noAsk={harness} />
                )}
              </li>
            );
          })}
        </ul>
      )}
    </Section>
  );
}
