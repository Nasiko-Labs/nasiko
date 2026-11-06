// Base UI unmounts a modal's backdrop when the popup's exit animation ends, and tw-animate's fill mode is none:
// a backdrop that finishes fading first snaps back to full black/50 until then, which reads as the page flickering
// on close. So each modal's backdrop must close over exactly the popup's duration.
import { render } from "@testing-library/react"
import type { ReactNode } from "react"
import { describe, expect, it } from "vitest"
import { AlertDialog, AlertDialogContent, AlertDialogTitle } from "./alert-dialog"
import { Dialog, DialogContent, DialogTitle } from "./dialog"
import { Sheet, SheetContent, SheetTitle } from "./sheet"

/** The duration that applies while closing: a `data-closed:` one wins over a plain one. */
function exitDuration(slot: string) {
  const tokens = document.querySelector(`[data-slot="${slot}"]`)!.className.split(/\s+/)
  return (
    tokens.find((t) => t.startsWith("data-closed:duration-"))?.replace("data-closed:", "") ??
    tokens.find((t) => t.startsWith("duration-")) ??
    "tw-animate default"
  )
}

const cases: [string, ReactNode][] = [
  [
    "dialog",
    <Dialog open>
      <DialogContent>
        <DialogTitle>t</DialogTitle>
      </DialogContent>
    </Dialog>,
  ],
  [
    "alert-dialog",
    <AlertDialog open>
      <AlertDialogContent>
        <AlertDialogTitle>t</AlertDialogTitle>
      </AlertDialogContent>
    </AlertDialog>,
  ],
  [
    "sheet",
    <Sheet open>
      <SheetContent>
        <SheetTitle>t</SheetTitle>
      </SheetContent>
    </Sheet>,
  ],
]

describe("modal exit timing", () => {
  it.each(cases)("%s: the backdrop closes over the popup's duration", (slot, ui) => {
    render(ui)
    expect(exitDuration(`${slot}-overlay`)).toBe(exitDuration(`${slot}-content`))
  })
})
