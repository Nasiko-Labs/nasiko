// Ported from nasiko-ui-lab@1b99bdda common/src/features/deploy/components/DeployAgentButton.tsx. Local edits:
// `Button asChild` + `Link` -> `ButtonLink` (agents/components/ButtonLink.tsx: a navigation-port `Link` anchor with the button
// classes) to `hrefFor('/deploy', { name })`; optional props admit `undefined` (exactOptionalPropertyTypes);
// Prettier formatting.
"use client";

/**
 * The one "Deploy an agent" entry point (plans/feat-deploy.md §7): every CLI-only empty state gets it, and the CLI steps
 * stay beside it as the alternative. `name` pre-fills the form (an agent's "not deployed" state).
 */
import { Upload } from "lucide-react";
import { hrefFor } from "@nasiko/app-core/url-state";
import { ButtonLink } from "../../agents/components/ButtonLink";
import { cn } from "@nasiko/ui/cn";
import { copy } from "../copy";

export function DeployAgentButton({
  name,
  variant = "default",
  size = "sm",
  className,
}: {
  name?: string | undefined;
  variant?: "default" | "outline" | undefined;
  size?: "sm" | "default" | undefined;
  className?: string | undefined;
}) {
  return (
    <ButtonLink
      href={hrefFor("/deploy", name ? { name } : {})}
      variant={variant}
      size={size}
      className={cn("pointer-coarse:min-h-11", className)}
    >
      <Upload aria-hidden /> {copy.entry.label}
    </ButtonLink>
  );
}
