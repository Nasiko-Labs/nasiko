"use client"

// Local edits (stage D): the pre-D shadcn `progress` on Base UI. The root is the bar and the indicator its direct
// child, as Radix's were, so callers' classes size the bar and their `[&>[data-slot=progress-indicator]]` selectors
// reach the fill. Base UI sets the fill's width inline (Radix translated it); with no value the fill is empty, as
// Radix's was (`translateX(-100%)`), so an indeterminate bar is the track alone.
import { Progress as ProgressPrimitive } from "@base-ui/react/progress"
import { cn } from "../lib/utils"

function Progress({ className, ...props }: ProgressPrimitive.Root.Props) {
  return (
    <ProgressPrimitive.Root
      data-slot="progress"
      className={cn(
        "relative h-2 w-full overflow-hidden rounded-full bg-primary/20",
        className
      )}
      {...props}
    >
      <ProgressPrimitive.Indicator
        data-slot="progress-indicator"
        className="h-full w-full flex-1 bg-primary transition-all data-indeterminate:w-0"
      />
    </ProgressPrimitive.Root>
  )
}

export { Progress }
