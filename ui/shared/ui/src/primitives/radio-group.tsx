"use client"

// Local edit (stage D): the root is a native `<button>` (`nativeButton`), as Radix's was: a `<label htmlFor>` names it
// and loses to its own `aria-label` (Base UI's span takes the label's text through `aria-labelledby`, which wins),
// and `disabled` is the button's own attribute.

// Ported from nasiko-ui-lab@1b99bdda common/src/components/ui/radio-group.tsx (shadcn new-york-v4 with the lab's
// edits: `text-primary-text` accent, the app's 2 px focus ring). Local edits: Base UI `RadioGroup` + `Radio`
// instead of Radix (`RadioGroupPrimitive.Item` → `Radio.Root`, `Indicator` → `Radio.Indicator`); the item class
// string is the lab's, with Radix's `data-[state=checked]` hooks unused (the lab styles none); `cn` from "../lib/utils".
// The root centres the indicator itself (Base UI's root is a span; Radix's was a button, which centres by default).
import { Radio as RadioPrimitive } from "@base-ui/react/radio"
import { RadioGroup as RadioGroupPrimitive } from "@base-ui/react/radio-group"
import { cn } from "../lib/utils"
import { CircleIcon } from "lucide-react"

function RadioGroup({ className, ...props }: RadioGroupPrimitive.Props) {
  return (
    <RadioGroupPrimitive
      data-slot="radio-group"
      className={cn("grid gap-3", className)}
      {...props}
    />
  )
}

function RadioGroupItem({ className, ...props }: RadioPrimitive.Root.Props) {
  return (
    <RadioPrimitive.Root
      data-slot="radio-group-item"
      className={cn(
        "inline-flex aspect-square size-4 shrink-0 items-center justify-center rounded-full border border-input text-primary-text shadow-xs transition-[color,box-shadow] outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 data-disabled:cursor-not-allowed data-disabled:opacity-50 dark:bg-input/30 dark:aria-invalid:ring-destructive/40",
        className
      )}
      nativeButton
      render={<button type="button" />}
      {...props}
    >
      {/* Base UI renders the root as a <span>, not a <button>, so nothing centers its content by default: the root is
          the flex container, and the dot sits in normal flow inside it. (An absolutely positioned dot inside a 0×0
          indicator centred itself on the root's top edge and drew above the ring.) */}
      <RadioPrimitive.Indicator
        data-slot="radio-group-indicator"
        className="flex items-center justify-center"
      >
        <CircleIcon className="size-2 fill-primary" />
      </RadioPrimitive.Indicator>
    </RadioPrimitive.Root>
  )
}

export { RadioGroup, RadioGroupItem }
