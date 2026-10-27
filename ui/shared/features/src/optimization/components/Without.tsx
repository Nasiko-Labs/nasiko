/**
 * "Without optimization" with the shared 2A tooltip (plans/feat-optimization-page.md R4C): the definition is said once,
 * under the chart; every other place names the baseline through this.
 */
import { Tooltip, TooltipContent, TooltipTrigger } from '@nasiko/ui/primitives/tooltip'
import { copy } from '../copy'

export function Without({ label }: { label: string }) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <span
            // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- focusable only so keyboard users can open the baseline tooltip; it has no action, so no button role
            tabIndex={0}
            className="underline decoration-dotted underline-offset-4"
          />
        }
      >
        {label}
      </TooltipTrigger>
      <TooltipContent className="max-w-xs">{copy.preview.baselineTip}</TooltipContent>
    </Tooltip>
  )
}
