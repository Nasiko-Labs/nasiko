// fidelity: nasiko-ui-lab@1b99bdda: class strings are the lab's `select` (Radix vars → Base UI's (`--available-height`, `--anchor-width`, `--transform-origin`); the lab's popper `translate-*-1` gap is the Positioner's `sideOffset` 4 instead; `h-[var(--radix-select-trigger-height)]` has no Base UI counterpart and is dropped; the scroll arrows keep base-nova's `top-0/bottom-0 z-10 w-full bg-popover` because Base UI overlays them on the list); Base UI internals kept.
// Local edits (ported from nasiko-ui-lab@1b99bdda common/src/components/ui/select.tsx, re-apply after any re-vendor):
// - focus is the one app style: a 2 px `--ring` with a 2 px page-coloured offset (DESIGN.md "Focus"), not the 3 px glow.
// - `SelectContent` drops below the trigger like every other dropdown (`alignItemWithTrigger` false, `align` "start"),
//   instead of item-aligned (covering the trigger and jumping to the selected item): the lab's `position="popper"`.
"use client"

import * as React from "react"
import { Select as SelectPrimitive } from "@base-ui/react/select"
import { cn } from "../lib/utils"

import { CheckIcon, ChevronDownIcon, ChevronUpIcon } from "lucide-react"
import { usePortalContainer } from "../motion/portal"

/**
 * Base UI reports `null` when a select is cleared. This tree's selects always hold a value, and their handlers take one
 * (Radix never sent null), so a null change is dropped here: a local edit (stage D).
 */
function Select<Value, Multiple extends boolean | undefined = false>({
  onValueChange,
  ...props
}: Omit<SelectPrimitive.Root.Props<Value, Multiple>, "onValueChange"> & {
  onValueChange?: (
    value: Exclude<
      Parameters<
        NonNullable<
          SelectPrimitive.Root.Props<Value, Multiple>["onValueChange"]
        >
      >[0],
      null
    >,
    eventDetails: SelectPrimitive.Root.ChangeEventDetails
  ) => void
}) {
  return (
    <SelectPrimitive.Root<Value, Multiple>
      {...props}
      items={props.items ?? itemsOf(props.children)}
      onValueChange={(value, details) => {
        if (value !== null) onValueChange?.(value as never, details)
      }}
    />
  )
}

/**
 * Radix's trigger showed the picked item's text; Base UI's shows `items[value]`, else the raw value. So when a caller
 * passes no `items`, they are read off the `SelectItem`s written among the children: a local edit (stage D).
 * ponytail: an item rendered by another component isn't seen (the trigger shows its raw value); pass `items` there.
 */
function itemsOf(
  node: React.ReactNode,
  found: Record<string, React.ReactNode> = {}
) {
  React.Children.forEach(node, (child) => {
    if (
      !React.isValidElement<{ value?: unknown; children?: React.ReactNode }>(
        child
      )
    )
      return
    if (child.type === SelectItem && typeof child.props.value === "string")
      found[child.props.value] = child.props.children
    else itemsOf(child.props.children, found)
  })
  return found
}

function SelectGroup({ className, ...props }: SelectPrimitive.Group.Props) {
  return (
    <SelectPrimitive.Group
      data-slot="select-group"
      className={cn(className)}
      {...props}
    />
  )
}

function SelectValue({ className, ...props }: SelectPrimitive.Value.Props) {
  return (
    <SelectPrimitive.Value
      data-slot="select-value"
      className={cn(className)}
      {...props}
    />
  )
}

function SelectTrigger({
  className,
  size = "default",
  children,
  ...props
}: SelectPrimitive.Trigger.Props & {
  size?: "sm" | "default"
}) {
  return (
    <SelectPrimitive.Trigger
      data-slot="select-trigger"
      data-size={size}
      className={cn(
        "flex w-fit items-center justify-between gap-2 rounded-md border border-input bg-transparent px-3 py-2 text-sm whitespace-nowrap shadow-xs transition-[color,box-shadow] outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 data-placeholder:text-muted-foreground data-[size=default]:h-9 data-[size=sm]:h-8 *:data-[slot=select-value]:line-clamp-1 *:data-[slot=select-value]:flex *:data-[slot=select-value]:items-center *:data-[slot=select-value]:gap-2 dark:bg-input/30 dark:hover:bg-input/50 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4 [&_svg:not([class*='text-'])]:text-muted-foreground",
        className
      )}
      {...props}
    >
      {children}
      <SelectPrimitive.Icon
        render={<ChevronDownIcon className="size-4 opacity-50" />}
      />
    </SelectPrimitive.Trigger>
  )
}

function SelectContent({
  className,
  children,
  side = "bottom",
  sideOffset = 4,
  align = "start",
  alignOffset = 0,
  alignItemWithTrigger = false,
  ...props
}: SelectPrimitive.Popup.Props &
  Pick<
    SelectPrimitive.Positioner.Props,
    "align" | "alignOffset" | "side" | "sideOffset" | "alignItemWithTrigger"
  >) {
  return (
    <SelectPrimitive.Portal container={usePortalContainer()}>
      <SelectPrimitive.Positioner
        side={side}
        sideOffset={sideOffset}
        align={align}
        alignOffset={alignOffset}
        alignItemWithTrigger={alignItemWithTrigger}
        className="isolate z-50"
      >
        <SelectPrimitive.Popup
          data-slot="select-content"
          data-align-trigger={alignItemWithTrigger}
          className={cn(
            "relative z-50 max-h-(--available-height) min-w-[8rem] origin-(--transform-origin) overflow-x-hidden overflow-y-auto rounded-md border bg-popover text-popover-foreground shadow-md data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95 data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-[align-trigger=true]:animate-none data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2",
            className
          )}
          {...props}
        >
          <SelectScrollUpButton />
          <SelectPrimitive.List className="w-full min-w-(--anchor-width) scroll-my-1 p-1">
            {children}
          </SelectPrimitive.List>
          <SelectScrollDownButton />
        </SelectPrimitive.Popup>
      </SelectPrimitive.Positioner>
    </SelectPrimitive.Portal>
  )
}

function SelectLabel({
  className,
  ...props
}: SelectPrimitive.GroupLabel.Props) {
  return (
    <SelectPrimitive.GroupLabel
      data-slot="select-label"
      className={cn("px-2 py-1.5 text-xs text-muted-foreground", className)}
      {...props}
    />
  )
}

function SelectItem({
  className,
  children,
  ...props
}: SelectPrimitive.Item.Props) {
  return (
    <SelectPrimitive.Item
      data-slot="select-item"
      className={cn(
        "relative flex w-full cursor-default items-center gap-2 rounded-sm py-1.5 pr-8 pl-2 text-sm outline-hidden select-none focus:bg-accent focus:text-accent-foreground data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4 [&_svg:not([class*='text-'])]:text-muted-foreground *:[span]:last:flex *:[span]:last:items-center *:[span]:last:gap-2",
        className
      )}
      {...props}
    >
      <SelectPrimitive.ItemText>{children}</SelectPrimitive.ItemText>
      <SelectPrimitive.ItemIndicator
        render={
          <span
            data-slot="select-item-indicator"
            className="absolute right-2 flex size-3.5 items-center justify-center"
          />
        }
      >
        <CheckIcon className="size-4" />
      </SelectPrimitive.ItemIndicator>
    </SelectPrimitive.Item>
  )
}

function SelectSeparator({
  className,
  ...props
}: SelectPrimitive.Separator.Props) {
  return (
    <SelectPrimitive.Separator
      data-slot="select-separator"
      className={cn("pointer-events-none -mx-1 my-1 h-px bg-border", className)}
      {...props}
    />
  )
}

function SelectScrollUpButton({
  className,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.ScrollUpArrow>) {
  return (
    <SelectPrimitive.ScrollUpArrow
      data-slot="select-scroll-up-button"
      className={cn(
        "top-0 z-10 flex w-full cursor-default items-center justify-center bg-popover py-1",
        className
      )}
      {...props}
    >
      <ChevronUpIcon className="size-4" />
    </SelectPrimitive.ScrollUpArrow>
  )
}

function SelectScrollDownButton({
  className,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.ScrollDownArrow>) {
  return (
    <SelectPrimitive.ScrollDownArrow
      data-slot="select-scroll-down-button"
      className={cn(
        "bottom-0 z-10 flex w-full cursor-default items-center justify-center bg-popover py-1",
        className
      )}
      {...props}
    >
      <ChevronDownIcon className="size-4" />
    </SelectPrimitive.ScrollDownArrow>
  )
}

export {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectScrollDownButton,
  SelectScrollUpButton,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
}
