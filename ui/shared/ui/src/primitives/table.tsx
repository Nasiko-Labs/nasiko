// fidelity: nasiko-ui-lab@1b99bdda: class strings are the lab's `table` (`[&>[role=checkbox]]:translate-y-[2px]` on head and cell); Base UI internals kept.
// Local edits (re-apply after any re-vendor):
// - the `overflow-x-auto` container becomes a named, keyboard-reachable scroll region while (and only while) the
//   table is wider than it (axe `scrollable-region-focusable`; WCAG 2.1.1): `tabIndex={0}`, `role="region"`,
//   `aria-label={scrollLabel ?? aria-label}`, and the lab's focus ring. A table that fits stays the lab's plain div,
//   with no extra tab stop. The lab's own table has the same gap; this is the fix, not a divergence in design.
"use client"

import * as React from "react"
import { cn } from "../lib/utils"

/** Whether `ref`'s box is narrower than its content, kept current on resize of either. */
function useOverflowsX(ref: React.RefObject<HTMLElement | null>): boolean {
  const [overflows, setOverflows] = React.useState(false)
  React.useEffect(() => {
    const el = ref.current
    if (!el) return
    const check = () => {
      setOverflows(el.scrollWidth > el.clientWidth)
    }
    check()
    if (typeof ResizeObserver !== "function") return
    const ro = new ResizeObserver(check)
    ro.observe(el)
    if (el.firstElementChild) ro.observe(el.firstElementChild)
    return () => {
      ro.disconnect()
    }
  }, [ref])
  return overflows
}

function Table({
  className,
  scrollLabel,
  ...props
}: React.ComponentProps<"table"> & {
  /** Names the horizontal scroll region when the table overflows; defaults to the table's `aria-label`. */
  scrollLabel?: string | undefined
}) {
  const ref = React.useRef<HTMLDivElement>(null)
  const overflows = useOverflowsX(ref)
  const label = scrollLabel ?? props["aria-label"]
  return (
    <div
      ref={ref}
      data-slot="table-container"
      className={cn(
        "relative w-full overflow-x-auto",
        overflows &&
          "focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background focus-visible:outline-none"
      )}
      {...(overflows
        ? {
            tabIndex: 0,
            ...(label ? { role: "region", "aria-label": label } : {}),
          }
        : {})}
    >
      <table
        data-slot="table"
        className={cn("w-full caption-bottom text-sm", className)}
        {...props}
      />
    </div>
  )
}

function TableHeader({ className, ...props }: React.ComponentProps<"thead">) {
  return (
    <thead
      data-slot="table-header"
      className={cn("[&_tr]:border-b", className)}
      {...props}
    />
  )
}

function TableBody({ className, ...props }: React.ComponentProps<"tbody">) {
  return (
    <tbody
      data-slot="table-body"
      className={cn("[&_tr:last-child]:border-0", className)}
      {...props}
    />
  )
}

function TableFooter({ className, ...props }: React.ComponentProps<"tfoot">) {
  return (
    <tfoot
      data-slot="table-footer"
      className={cn(
        "border-t bg-muted/50 font-medium [&>tr]:last:border-b-0",
        className
      )}
      {...props}
    />
  )
}

function TableRow({ className, ...props }: React.ComponentProps<"tr">) {
  return (
    <tr
      data-slot="table-row"
      className={cn(
        "border-b transition-colors hover:bg-muted/50 has-aria-expanded:bg-muted/50 data-[state=selected]:bg-muted",
        className
      )}
      {...props}
    />
  )
}

function TableHead({ className, ...props }: React.ComponentProps<"th">) {
  return (
    <th
      data-slot="table-head"
      className={cn(
        "h-10 px-2 text-left align-middle font-medium whitespace-nowrap text-foreground [&:has([role=checkbox])]:pr-0 [&>[role=checkbox]]:translate-y-[2px]",
        className
      )}
      {...props}
    />
  )
}

function TableCell({ className, ...props }: React.ComponentProps<"td">) {
  return (
    <td
      data-slot="table-cell"
      className={cn(
        "p-2 align-middle whitespace-nowrap [&:has([role=checkbox])]:pr-0 [&>[role=checkbox]]:translate-y-[2px]",
        className
      )}
      {...props}
    />
  )
}

function TableCaption({
  className,
  ...props
}: React.ComponentProps<"caption">) {
  return (
    <caption
      data-slot="table-caption"
      className={cn("mt-4 text-sm text-muted-foreground", className)}
      {...props}
    />
  )
}

export {
  Table,
  TableHeader,
  TableBody,
  TableFooter,
  TableHead,
  TableRow,
  TableCell,
  TableCaption,
}
