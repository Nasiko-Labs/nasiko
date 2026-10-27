/**
 * A mechanism's name with a hover (and focus) tooltip that introduces its team name (PACMS) (user
 * decision 2026-10-05, plans/feat-optimization-page.md B12). Built like `Without` (the 2A tooltip).
 */
import { Tooltip, TooltipContent, TooltipTrigger } from '@nasiko/ui/primitives/tooltip'
import { copy } from '../copy'

export type CodenameKey = keyof typeof copy.codenames

export function Codename({ label, name }: { label: string; name: CodenameKey }) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <span
            // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- focusable only so keyboard users can open the introduction; it has no action, so no button role
            tabIndex={0}
            className="underline decoration-dotted underline-offset-4"
          />
        }
      >
        {label}
      </TooltipTrigger>
      <TooltipContent className="max-w-xs">{copy.codenames[name]}</TooltipContent>
    </Tooltip>
  )
}
