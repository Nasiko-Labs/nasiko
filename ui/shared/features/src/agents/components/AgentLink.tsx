// Ported from nasiko-ui-lab@1b99bdda common/src/features/agents/components/AgentLink.tsx (with `AgentLinkTo` from
// components/bits.tsx). Local edits: `AgentLinkTo` is inlined here (the rest of bits.tsx comes with the agents
// pages) on the navigation port's `Link` (`@nasiko/app-core/navigation`), `href` built from the lab's `/agents/$agentId` with `encodeURIComponent` (what the lab's
// router did for params); `cn` from `@nasiko/ui/cn`; optional props typed `| undefined`; Prettier formatting.
"use client";

/**
 * E2: an agent name elsewhere in the app links to its detail page. Resolves through the shared
 * directory by id first, then by raw name; a name several agents share, or an agent the
 * directory doesn't have (deleted, not visible, still loading), renders plain text.
 */
import Link from "@nasiko/app-core/navigation";
import type { ReactNode } from "react";
import { cn } from "@nasiko/ui/cn";
import { useAgentsDirectory } from "../api";

/** `fallback` replaces the plain-text form (null hides it, for a link that only makes sense as a link). */
export function AgentLink({
  id,
  name,
  children,
  className,
  fallback,
}: {
  id?: string | null | undefined;
  name?: string | null | undefined;
  children: ReactNode;
  className?: string | undefined;
  fallback?: ReactNode;
}) {
  const dir = useAgentsDirectory();
  const byName = name ? dir.byNameAll.get(name) : undefined;
  const target =
    (id ? dir.byId.get(id)?.id : undefined) ?? (byName?.length === 1 ? byName[0]?.id : undefined);
  if (!target)
    return fallback !== undefined ? fallback : <span className={className}>{children}</span>;
  return (
    <AgentLinkTo id={target} className={className}>
      {children}
    </AgentLinkTo>
  );
}

export function AgentLinkTo({
  id,
  children,
  className,
}: {
  id: string;
  children: ReactNode;
  className?: string | undefined;
}) {
  return (
    <Link
      href={`/agents/${encodeURIComponent(id)}`}
      className={cn("underline-offset-4 hover:underline", className)}
    >
      {children}
    </Link>
  );
}
