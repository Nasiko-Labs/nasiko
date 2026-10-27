// fidelity: nasiko-ui-lab@1b99bdda: class strings are the lab's `toggle-group` (default `spacing` 0 as the lab's, the base-nova `orientation` prop stays, with only its two `data-vertical:` classes kept so vertical groups still stack (inert for the lab's horizontal default)); Base UI internals kept.
// a11y: a single-choice group (no `multiple`, every use in the lab's `type="single"`) is a radio group, as the lab's
// Radix group was (items `role="radio"` + `aria-checked`): Base UI's own group is pressed buttons. Keyboard follows the
// WAI-ARIA radio group pattern: one tab stop (the checked item), Arrow keys move AND select, Home/End, wrap unless
// `loopFocus={false}`; the checked item cannot be unchecked. A `multiple` group keeps Base UI's pressed buttons.
"use client"

import * as React from "react"
import { Toggle as TogglePrimitive } from "@base-ui/react/toggle"
import { ToggleGroup as ToggleGroupPrimitive } from "@base-ui/react/toggle-group"
import { type VariantProps } from "class-variance-authority"
import { cn } from "../lib/utils"

import { toggleVariants } from "./toggle"

const ToggleGroupContext = React.createContext<
  VariantProps<typeof toggleVariants> & {
    spacing?: number
    orientation?: "horizontal" | "vertical"
    /** Single choice: radio semantics (see the file header). */
    single?: boolean
    /** Whether any item is selected (the tab stop is then the checked item). */
    hasSelection?: boolean
  }
>({
  size: "default",
  variant: "default",
  spacing: 0,
  orientation: "horizontal",
})

const ARROWS: Record<string, number> = {
  ArrowRight: 1,
  ArrowDown: 1,
  ArrowLeft: -1,
  ArrowUp: -1,
}

function ToggleGroup({
  className,
  variant,
  size,
  spacing = 0,
  orientation = "horizontal",
  children,
  multiple,
  value,
  defaultValue,
  onValueChange,
  onKeyDown,
  loopFocus = true,
  ...props
}: ToggleGroupPrimitive.Props &
  VariantProps<typeof toggleVariants> & {
    spacing?: number
    orientation?: "horizontal" | "vertical"
  }) {
  const single = !multiple
  const [uncontrolled, setUncontrolled] = React.useState<
    NonNullable<ToggleGroupPrimitive.Props["value"]>
  >(() => defaultValue ?? [])
  const selected = value ?? uncontrolled
  return (
    <ToggleGroupPrimitive
      role={single ? "radiogroup" : undefined}
      multiple={multiple}
      value={selected}
      loopFocus={loopFocus}
      onValueChange={(next, details) => {
        // A radio cannot be unchecked: pressing the checked item again changes nothing (the lab's handlers all
        // ignored the empty value Radix sent for it).
        if (single && next.length === 0 && selected.length > 0) return
        if (value === undefined) setUncontrolled(next)
        onValueChange?.(next, details)
      }}
      onKeyDown={(e) => {
        onKeyDown?.(e)
        if (!single || e.defaultPrevented) return
        const items = [
          ...e.currentTarget.querySelectorAll<HTMLElement>(
            '[data-slot="toggle-group-item"]'
          ),
        ].filter(
          (el) =>
            !(el as HTMLButtonElement).disabled &&
            el.getAttribute("aria-disabled") !== "true"
        )
        const at = items.indexOf(document.activeElement as HTMLElement)
        if (at < 0) return
        let to: number
        if (e.key === "Home") to = 0
        else if (e.key === "End") to = items.length - 1
        else if (e.key in ARROWS) {
          to = at + (ARROWS[e.key] ?? 0)
          if (to < 0 || to >= items.length) {
            if (!loopFocus) return
            to = (to + items.length) % items.length
          }
        } else return
        e.preventDefault()
        ;(e as { preventBaseUIHandler?: () => void }).preventBaseUIHandler?.()
        const next = items[to]
        if (!next) return
        next.focus()
        if (next.getAttribute("aria-checked") !== "true") next.click()
      }}
      data-slot="toggle-group"
      data-variant={variant}
      data-size={size}
      data-spacing={spacing}
      data-orientation={orientation}
      style={{ "--gap": spacing } as React.CSSProperties}
      className={cn(
        "group/toggle-group flex w-fit items-center gap-[--spacing(var(--gap))] rounded-md data-vertical:flex-col data-vertical:items-stretch data-[spacing=default]:data-[variant=outline]:shadow-xs",
        className
      )}
      {...props}
    >
      <ToggleGroupContext.Provider
        value={{
          variant,
          size,
          spacing,
          orientation,
          single,
          hasSelection: selected.length > 0,
        }}
      >
        {children}
      </ToggleGroupContext.Provider>
    </ToggleGroupPrimitive>
  )
}

function ToggleGroupItem({
  className,
  children,
  variant = "default",
  size = "default",
  ...props
}: TogglePrimitive.Props & VariantProps<typeof toggleVariants>) {
  const context = React.useContext(ToggleGroupContext)

  return (
    <TogglePrimitive
      data-slot="toggle-group-item"
      data-variant={context.variant || variant}
      data-size={context.size || size}
      data-spacing={context.spacing}
      className={cn(
        toggleVariants({
          variant: context.variant || variant,
          size: context.size || size,
        }),
        "w-auto min-w-0 shrink-0 px-3 focus:z-10 focus-visible:z-10",
        "data-[spacing=0]:rounded-none data-[spacing=0]:shadow-none data-[spacing=0]:first:rounded-l-md data-[spacing=0]:last:rounded-r-md data-[spacing=0]:data-[variant=outline]:border-l-0 data-[spacing=0]:data-[variant=outline]:first:border-l",
        className
      )}
      {...props}
      render={
        context.single
          ? (p, state) => (
              <button
                {...p}
                role="radio"
                aria-checked={state.pressed}
                aria-pressed={undefined}
                tabIndex={
                  context.hasSelection ? (state.pressed ? 0 : -1) : p.tabIndex
                }
              />
            )
          : undefined
      }
    >
      {children}
    </TogglePrimitive>
  )
}

export { ToggleGroup, ToggleGroupItem }
