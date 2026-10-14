// Ported from nasiko-ui-lab@1b99bdda common/src/features/optimization/components/Without.tsx. Local edits:
// `TooltipTrigger asChild` -> `render={<span …/>}` (Base UI useRender; the same focusable span, as Codename.tsx);
// Prettier formatting.
/**
 * "Without optimization" with the shared 2A tooltip (plans/feat-optimization-page.md R4C): the definition is said once,
 * under the chart; every other place names the baseline through this.
 */
import { Tooltip, TooltipContent, TooltipTrigger } from "@nasiko/ui/primitives";
import { copy } from "../copy";

export function Without({ label }: { label: string }) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={<span tabIndex={0} className="underline decoration-dotted underline-offset-4" />}
      >
        {label}
      </TooltipTrigger>
      <TooltipContent className="max-w-xs">{copy.preview.baselineTip}</TooltipContent>
    </Tooltip>
  );
}
