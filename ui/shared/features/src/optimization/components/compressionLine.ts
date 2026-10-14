// Ported from nasiko-ui-lab@1b99bdda common/src/features/optimization/components/compressionLine.ts. Local edits:
// `meQuery`/`tiersQuery` are `meQueryOptions(scope)`/`tiersQuery(scope)`; Prettier formatting.
/**
 * The lead's history compression sentence (plans/feat-optimization-page.md C1, P10): `compressionState` over your own
 * agents, as Settings' old Compression row read it. Null while it loads, when the list failed (Needs attention says
 * so), and when some agents are off (Needs attention names and fixes them): each fact is said once.
 */
import { useQuery } from "@tanstack/react-query";
import { useEdition } from "@nasiko/app-core/edition/edition-context";
import { useOwnedAgents } from "../../agents/api";
import { AGENTS_MAX_PAGES, AGENTS_PAGE } from "../../agents/tuning";
import { meQueryOptions, useProductScope } from "@nasiko/data-access/product";
import { tiersQuery } from "../api";
import { copy } from "../copy";
import { compressionState, toOwnedAgent } from "../logic";

export function useCompressionLine(): string | null {
  const scope = useProductScope();
  const me = useQuery(meQueryOptions(scope));
  const owned = useOwnedAgents(me.data?.sub);
  const tiers = useQuery(tiersQuery(scope));
  const edition = useEdition();
  if (!owned.data || tiers.isPending) return null;
  const state = compressionState(owned.data.map(toOwnedAgent), {
    ...(tiers.data?.compress_history === undefined
      ? {}
      : { serverOn: tiers.data.compress_history }),
    // EE seeds a hidden internal agent for the first superuser; the opt-in counts it (CX-3b).
    hiddenMayCount: edition !== "oss" && me.data?.is_superuser === true,
    capped: owned.data.length >= AGENTS_PAGE * AGENTS_MAX_PAGES,
  });
  return state.reason === "some-off" ? null : copy.lead.compression[state.reason];
}
