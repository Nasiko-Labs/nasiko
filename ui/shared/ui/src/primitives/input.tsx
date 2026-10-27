// fidelity: nasiko-ui-lab@1b99bdda: class strings are the lab's `input` (); Base UI internals kept.
// Local edits (ported from nasiko-ui-lab@1b99bdda common/src/components/ui/input.tsx, re-apply after any re-vendor):
// - focus is the one app style: a 2 px `--ring` with a 2 px page-coloured offset (DESIGN.md "Focus"), not the 3 px glow.
import * as React from "react"
import { Input as InputPrimitive } from "@base-ui/react/input"
import { cn } from "../lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <InputPrimitive
      type={type}
      data-slot="input"
      className={cn(
        "h-9 w-full min-w-0 rounded-md border border-input bg-transparent px-3 py-1 text-base shadow-xs transition-[color,box-shadow] outline-none selection:bg-primary selection:text-primary-foreground file:inline-flex file:h-7 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm dark:bg-input/30",
        "focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
        "aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40",
        className
      )}
      {...props}
    />
  )
}

export { Input }
