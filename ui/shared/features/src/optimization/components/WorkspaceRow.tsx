// Ported from nasiko-ui-lab@1b99bdda common/src/features/optimization/components/WorkspaceRow.tsx. Local edits:
// `Button asChild` around a router `Link` -> `ButtonLink` (agents/components/ButtonLink.tsx: a navigation-port `Link` anchor
// with `buttonVariants()`; Base UI's Button would add `role="button"` to it), its `href`
// `hrefFor('/settings', { section })` for the workspace page; optional props typed `| undefined`; Prettier formatting.
/**
 * One row of /optimization's Workspace footer (plans/feat-optimization-page.md R1B, R3A, R5B): what is set for the
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
    <li className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
      <span className="text-muted-foreground">{label}:</span>
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
        <span>{value}</span>
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
