// fidelity: nasiko-ui-lab@1b99bdda: class strings are the lab's `sheet` (220 ms open / 160 ms close, reduced motion keeps only the fade, the close button is the lab's plain `SheetPrimitive.Close` with its classes, not a Button, `data-[state=…]` → `data-open:`/`data-closed:`); Base UI internals kept.
"use client"

import * as React from "react"
import { Dialog as SheetPrimitive } from "@base-ui/react/dialog"
import { cn } from "../lib/utils"

import { XIcon } from "lucide-react"
import { usePortalContainer } from "../motion/portal"
import { focusProp, type AutoFocusHandler } from "./radix-compat"

function Sheet({ ...props }: SheetPrimitive.Root.Props) {
  return <SheetPrimitive.Root data-slot="sheet" {...props} />
}

function SheetTrigger({ ...props }: SheetPrimitive.Trigger.Props) {
  return <SheetPrimitive.Trigger data-slot="sheet-trigger" {...props} />
}

function SheetClose({ ...props }: SheetPrimitive.Close.Props) {
  return <SheetPrimitive.Close data-slot="sheet-close" {...props} />
}

function SheetPortal({ ...props }: SheetPrimitive.Portal.Props) {
  return (
    <SheetPrimitive.Portal
      container={usePortalContainer()}
      data-slot="sheet-portal"
      {...props}
    />
  )
}

function SheetOverlay({ className, ...props }: SheetPrimitive.Backdrop.Props) {
  return (
    <SheetPrimitive.Backdrop
      data-slot="sheet-overlay"
      className={cn(
        "fixed inset-0 z-50 bg-black/50 data-closed:animate-out data-closed:duration-[160ms] data-closed:fade-out-0 data-open:animate-in data-open:duration-[220ms] data-open:fade-in-0",
        className
      )}
      {...props}
    />
  )
}

function SheetContent({
  onOpenAutoFocus,
  onCloseAutoFocus,
  className,
  children,
  side = "right",
  showCloseButton = true,
  ...props
}: (SheetPrimitive.Popup.Props & {
  side?: "top" | "right" | "bottom" | "left"
  showCloseButton?: boolean
}) & {
  onOpenAutoFocus?: AutoFocusHandler
  onCloseAutoFocus?: AutoFocusHandler
}) {
  return (
    <SheetPortal>
      <SheetOverlay />
      <SheetPrimitive.Popup
        data-slot="sheet-content"
        data-side={side}
        className={cn(
          // Timings are motion.ts panelIn / panelOut (checked by motion.test.ts). Reduced motion keeps
          // the fade and drops the slide (plans/feat-app-shell.md §5.4).
          "fixed z-50 flex flex-col gap-4 bg-background shadow-lg ease-out data-closed:animate-out data-closed:duration-[160ms] data-open:animate-in data-open:duration-[220ms]",
          "motion-reduce:[--tw-enter-translate-x:0]! motion-reduce:[--tw-enter-translate-y:0]! motion-reduce:[--tw-exit-translate-x:0]! motion-reduce:[--tw-exit-translate-y:0]! motion-reduce:data-closed:fade-out-0 motion-reduce:data-open:fade-in-0",
          side === "right" &&
            "inset-y-0 right-0 h-full w-3/4 border-l data-closed:slide-out-to-right data-open:slide-in-from-right sm:max-w-sm",
          side === "left" &&
            "inset-y-0 left-0 h-full w-3/4 border-r data-closed:slide-out-to-left data-open:slide-in-from-left sm:max-w-sm",
          side === "top" &&
            "inset-x-0 top-0 h-auto border-b data-closed:slide-out-to-top data-open:slide-in-from-top",
          side === "bottom" &&
            "inset-x-0 bottom-0 h-auto border-t data-closed:slide-out-to-bottom data-open:slide-in-from-bottom",
          className
        )}
        {...props}
        initialFocus={focusProp(onOpenAutoFocus) ?? props.initialFocus}
        finalFocus={focusProp(onCloseAutoFocus) ?? props.finalFocus}
      >
        {children}
        {showCloseButton && (
          <SheetPrimitive.Close
            data-slot="sheet-close"
            className="absolute top-2 right-2 inline-flex size-8 items-center justify-center rounded-xs opacity-70 ring-offset-background transition-opacity hover:opacity-100 focus:ring-2 focus:ring-ring focus:ring-offset-2 focus:outline-hidden disabled:pointer-events-none data-open:bg-secondary pointer-coarse:top-1.5 pointer-coarse:right-1.5 pointer-coarse:size-11"
          >
            <XIcon className="size-4" />
            <span className="sr-only">Close</span>
          </SheetPrimitive.Close>
        )}
      </SheetPrimitive.Popup>
    </SheetPortal>
  )
}

function SheetHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="sheet-header"
      className={cn("flex flex-col gap-1.5 p-4", className)}
      {...props}
    />
  )
}

function SheetFooter({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="sheet-footer"
      className={cn("mt-auto flex flex-col gap-2 p-4", className)}
      {...props}
    />
  )
}

function SheetTitle({ className, ...props }: SheetPrimitive.Title.Props) {
  return (
    <SheetPrimitive.Title
      data-slot="sheet-title"
      className={cn("font-semibold text-foreground", className)}
      {...props}
    />
  )
}

function SheetDescription({
  className,
  ...props
}: SheetPrimitive.Description.Props) {
  return (
    <SheetPrimitive.Description
      data-slot="sheet-description"
      className={cn("text-sm text-muted-foreground", className)}
      {...props}
    />
  )
}

export {
  Sheet,
  SheetTrigger,
  SheetClose,
  SheetContent,
  SheetHeader,
  SheetFooter,
  SheetTitle,
  SheetDescription,
}
