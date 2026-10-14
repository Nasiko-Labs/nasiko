// Ported from nasiko-ui-lab@1b99bdda common/src/features/optimization/components/Codename.tsx. Local edits:
// `TooltipTrigger asChild` -> `render={<span …/>}` (Base UI useRender; the same focusable span); Prettier
// formatting.
"use client";

/**
 * A mechanism's name with a hover (and focus) tooltip that introduces its team name: PACMS, Caveman, Ponytail (user
 * decision 2026-10-05, plans/feat-optimization-page.md B12). Built like `Without` (the 2A tooltip).
 */
import { Tooltip, TooltipContent, TooltipTrigger } from "@nasiko/ui/primitives";
import { copy } from "../copy";

export type CodenameKey = keyof typeof copy.codenames;

export function Codename({ label, name }: { label: string; name: CodenameKey }) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={<span tabIndex={0} className="underline decoration-dotted underline-offset-4" />}
      >
        {label}
      </TooltipTrigger>
      <TooltipContent className="max-w-xs">{copy.codenames[name]}</TooltipContent>
    </Tooltip>
  );
}
