// Ported from nasiko-ui-lab@1b99bdda common/src/features/flows/components/OpenFlowLink.tsx. Local edits: the
// `<Button asChild><Link to="/flows/$flowId">` is a `ButtonLink` (../../agents/components/ButtonLink.tsx, the
// destination's form of it) to `/flows/<encoded id>`; optional props admit `undefined` (exactOptionalPropertyTypes);
// Prettier formatting.
/**
 * "Open flow" (plans/feat-flows.md F19): one link into a flow from wherever its request shows up (the trace page,
 * a Sessions row, chat's Activity, a workflow run step). A flow's id is its trace id, so the link goes straight to
 * `/flows/<id>`; an id with no flow (a trace that never went through the proxy) lands on the flow page's not-found
 * state, which says why.
 */
import { Network } from "lucide-react";
import { cn } from "@nasiko/ui/cn";
import { ButtonLink } from "../../agents/components/ButtonLink";
import { copy } from "../copy";

export function OpenFlowLink({
  flowId,
  latest = false,
  size = "sm",
  variant = "outline",
  className,
}: {
  flowId: string;
  /** "Open latest flow": the link picks one of several flows (a Sessions row with several traces). */
  latest?: boolean | undefined;
  size?: "sm" | "xs" | undefined;
  variant?: "outline" | "ghost" | undefined;
  className?: string | undefined;
}) {
  return (
    <ButtonLink
      href={`/flows/${encodeURIComponent(flowId)}`}
      size={size}
      variant={variant}
      className={cn("pointer-coarse:min-h-11", className)}
    >
      <Network className="size-3.5" aria-hidden /> {latest ? copy.openLatestFlow : copy.openFlow}
    </ButtonLink>
  );
}
