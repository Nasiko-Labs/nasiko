"use client"

// Local edit (stage D): the class strings are the pre-D new-york ones (Radix `data-[state=…]` → `data-checked`/
// `data-unchecked`), not base-nova's, so nothing on screen changes; that includes the app's 2 px focus ring.
// Local edit (stage D): the root is a native `<button>` (`nativeButton`), as Radix's was: a `<label htmlFor>` names it
// and loses to its own `aria-label` (Base UI's span takes the label's text through `aria-labelledby`, which wins),
// and `disabled` is the button's own attribute.

import { Switch as SwitchPrimitive } from "@base-ui/react/switch"
import { cn } from "../lib/utils"

function Switch({
  className,
  size = "default",
  ...props
}: SwitchPrimitive.Root.Props & {
  size?: "sm" | "default"
}) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      data-size={size}
      className={cn(
        "peer group/switch inline-flex shrink-0 items-center rounded-full border border-transparent shadow-xs transition-all outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50 data-[size=default]:h-[1.15rem] data-[size=default]:w-8 data-[size=sm]:h-3.5 data-[size=sm]:w-6 data-checked:bg-primary data-unchecked:bg-input dark:data-unchecked:bg-input/80",
        className
      )}
      nativeButton
      render={<button type="button" />}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className="pointer-events-none block rounded-full bg-background ring-0 transition-transform group-data-[size=default]/switch:size-4 group-data-[size=sm]/switch:size-3 data-checked:translate-x-[calc(100%-2px)] data-unchecked:translate-x-0 dark:data-checked:bg-primary-foreground dark:data-unchecked:bg-foreground"
      />
    </SwitchPrimitive.Root>
  )
}

export { Switch }
