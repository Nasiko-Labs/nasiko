// shadcn's base-nova `combobox` (ui.shadcn.com/r/styles/base-nova/combobox.json, fetched 2026-11-06) on Base UI's
// Autocomplete instead of its Combobox. Local edits (re-apply after any re-vendor):
// - Autocomplete parts: the input's text is the value, so any text is accepted, as with a `<datalist>` (whose popup
//   the browser draws, out of the theme's reach). A Combobox only commits a listed item.
// - lucide icons in place of the registry's IconPlaceholder; `cn` from ../lib/utils.
// - The popup portals into `usePortalContainer()` (inside a sheet, as every popup here).
// - Only the parts in use: no chips, clear, groups, collection, separator or item check (an autocomplete selects
//   nothing; it fills the input).
// - The chevron is a pointer convenience, `aria-hidden` and out of the tab order (`tabIndex={-1}`): the input opens the
//   list itself (click, typing, ArrowDown), so a form keeps one tab stop per field, as with the `<datalist>` inputs
//   this replaces.
"use client"

import { Autocomplete as AutocompletePrimitive } from "@base-ui/react/autocomplete"
import { ChevronDownIcon } from "lucide-react"
import { cn } from "../lib/utils"
import { usePortalContainer } from "../motion/portal"

import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "./input-group"

const Autocomplete = AutocompletePrimitive.Root
/** Base UI's own matcher (`Intl.Collator`), to give the list and an `open` check the same rule. */
const useAutocompleteFilter = AutocompletePrimitive.useFilter

function AutocompleteInput({
  className,
  disabled = false,
  ...props
}: AutocompletePrimitive.Input.Props) {
  return (
    <InputGroup className={cn("w-auto", className)}>
      <AutocompletePrimitive.Input
        render={<InputGroupInput disabled={disabled} />}
        {...props}
      />
      <InputGroupAddon align="inline-end">
        <InputGroupButton
          size="icon-xs"
          variant="ghost"
          render={<AutocompletePrimitive.Trigger />}
          data-slot="input-group-button"
          className="data-pressed:bg-transparent"
          tabIndex={-1}
          aria-hidden
          disabled={disabled}
        >
          <ChevronDownIcon className="pointer-events-none size-4 text-muted-foreground" />
        </InputGroupButton>
      </InputGroupAddon>
    </InputGroup>
  )
}

function AutocompleteContent({
  className,
  side = "bottom",
  sideOffset = 6,
  align = "start",
  alignOffset = 0,
  ...props
}: AutocompletePrimitive.Popup.Props &
  Pick<
    AutocompletePrimitive.Positioner.Props,
    "side" | "align" | "sideOffset" | "alignOffset"
  >) {
  return (
    <AutocompletePrimitive.Portal container={usePortalContainer()}>
      <AutocompletePrimitive.Positioner
        side={side}
        sideOffset={sideOffset}
        align={align}
        alignOffset={alignOffset}
        className="isolate z-50"
      >
        <AutocompletePrimitive.Popup
          data-slot="autocomplete-content"
          className={cn(
            "relative max-h-(--available-height) w-(--anchor-width) max-w-(--available-width) origin-(--transform-origin) overflow-hidden rounded-lg bg-popover text-popover-foreground shadow-md ring-1 ring-foreground/10 duration-100 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95",
            className
          )}
          {...props}
        />
      </AutocompletePrimitive.Positioner>
    </AutocompletePrimitive.Portal>
  )
}

function AutocompleteList({
  className,
  ...props
}: AutocompletePrimitive.List.Props) {
  return (
    <AutocompletePrimitive.List
      data-slot="autocomplete-list"
      className={cn(
        "no-scrollbar max-h-[min(calc(--spacing(72)---spacing(9)),calc(var(--available-height)---spacing(9)))] scroll-py-1 overflow-y-auto overscroll-contain p-1 data-empty:p-0",
        className
      )}
      {...props}
    />
  )
}

function AutocompleteItem({
  className,
  ...props
}: AutocompletePrimitive.Item.Props) {
  return (
    <AutocompletePrimitive.Item
      data-slot="autocomplete-item"
      className={cn(
        "relative flex w-full cursor-default items-center gap-2 rounded-md px-1.5 py-1 text-sm outline-hidden select-none data-highlighted:bg-accent data-highlighted:text-accent-foreground data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        className
      )}
      {...props}
    />
  )
}

export {
  Autocomplete,
  useAutocompleteFilter,
  AutocompleteInput,
  AutocompleteContent,
  AutocompleteList,
  AutocompleteItem,
}
