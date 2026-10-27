"use client"

// Local edit (stage D): the class strings are the pre-D new-york ones (Radix `data-[state=checked]` → `data-checked`),
// not base-nova's, so nothing on screen changes; that includes the app's 2 px focus ring with a 2 px offset.
// Local edit (stage D): the root is a native `<button>` (`nativeButton`), as Radix's was: a `<label htmlFor>` names it
// and loses to its own `aria-label` (Base UI's span takes the label's text through `aria-labelledby`, which wins),
// and `disabled` is the button's own attribute.

import { Checkbox as CheckboxPrimitive } from "@base-ui/react/checkbox"
import { cn } from "../lib/utils"

import { CheckIcon } from "lucide-react"

function Checkbox({ className, ...props }: CheckboxPrimitive.Root.Props) {
  return (
    <CheckboxPrimitive.Root
      data-slot="checkbox"
      className={cn(
        "peer size-4 shrink-0 rounded-[4px] border border-input shadow-xs transition-shadow outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 data-checked:border-primary data-checked:bg-primary data-checked:text-primary-foreground dark:bg-input/30 dark:aria-invalid:ring-destructive/40 dark:data-checked:bg-primary",
        className
      )}
      nativeButton
      render={<button type="button" />}
      {...props}
    >
      <CheckboxPrimitive.Indicator
        data-slot="checkbox-indicator"
        className="grid place-content-center text-current transition-none [&>svg]:size-3.5"
      >
        <CheckIcon />
      </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Root>
  )
}

export { Checkbox }
