// Ported from nasiko-ui-lab@1b99bdda common/src/features/optimization/components/WorkspaceRow.tsx. Local edits:
// `Button asChild` around a router `Link` -> `ButtonLink` (agents/components/ButtonLink.tsx: a navigation-port `Link` anchor
// with `buttonVariants()`; Base UI's Button would add `role="button"` to it), its `href`
// `hrefFor('/settings', { section })` for the workspace page; optional props typed `| undefined`; Prettier formatting.
// The redesign of TokenOps, Optimization, Overview and Harnesses (legacy ui/ on development, 3aac84f66: 8bf09bf29),
// merged into this port.
/**
 * One row of /optimization's Workspace card (plans/feat-optimization-page.md R1B, R3A, R5B): what is set for the
 * whole workspace, in one line, and where to change it. The core's tiers row and a layer's rows (EE: Organization
 * policy, through the `optimizationWorkspace` slot) are built from this, so they line up.
 */
import type { ReactNode } from "react";
import { hrefFor } from "@nasiko/app-core/url-state";
import { Button } from "@nasiko/ui/primitives";
import { ButtonLink } from "../../agents/components/ButtonLink";
import { Skeleton } from "@nasiko/ui/primitives";
import { copy } from "../copy";

export function WorkspaceRow({
  label,
  value,
  to,
  section,
  action,
  failed,
  onRetry,
}: {
  label: string;
  /** The setting in words; undefined while it loads. */
  value: ReactNode | undefined;
  /** Where it's edited (a Settings page, or `/settings` with its `?section=`). */
  to: "/settings" | "/settings/optimization-tiers";
  section?: string | undefined;
  action: string;
  /** The read failed: says so with Retry instead of a value. */
  failed?: string | undefined;
  onRetry?: (() => void) | undefined;
}) {
  return (
    // Stacked in the Workspace card: the setting's name, its value, then where to change it.
    <li className="flex flex-col items-start gap-1.5 border-t border-border py-3 text-sm first:border-t-0 first:pt-0 last:pb-0">
      <span className="text-xs text-muted-foreground">{label}</span>
      {failed ? (
        <span className="text-muted-foreground">
          {failed}{" "}
          <Button
            variant="link"
            size="sm"
            className="h-auto p-0 pointer-coarse:min-h-11"
            onClick={onRetry}
          >
            {copy.retry}
          </Button>
        </span>
      ) : value === undefined ? (
        <Skeleton aria-hidden className="h-4 w-56" />
      ) : (
        <span className="font-medium">{value}</span>
      )}
      <ButtonLink
        variant="outline"
        size="sm"
        className="pointer-coarse:min-h-11"
        href={to === "/settings" ? hrefFor(to, { section }) : to}
      >
        {action}
      </ButtonLink>
    </li>
  );
}
