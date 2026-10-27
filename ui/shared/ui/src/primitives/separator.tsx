"use client"

// Local edit (stage D): the pre-D sizing (`data-[orientation=…]`; a vertical rule is `h-full`, not base-nova's
// `self-stretch`), so nothing on screen changes.
import { Separator as SeparatorPrimitive } from "@base-ui/react/separator"
import { cn } from "../lib/utils"

function Separator({
  className,
  orientation = "horizontal",
  ...props
}: SeparatorPrimitive.Props) {
  return (
    <SeparatorPrimitive
      data-slot="separator"
      orientation={orientation}
      className={cn(
        "shrink-0 bg-border data-[orientation=horizontal]:h-px data-[orientation=horizontal]:w-full data-[orientation=vertical]:h-full data-[orientation=vertical]:w-px",
        className
      )}
      {...props}
    />
  )
}

export { Separator }
