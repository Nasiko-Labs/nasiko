// Tabs select on focus only when the keyboard moved focus there. Base UI also selects a tab focused any other
// way, and a closing dialog hands focus back to the tab that opened it: after a blocked tab change, that re-ran
// the change and reopened the dialog.
import { act, render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { useState } from "react"
import { describe, expect, it, vi } from "vitest"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "./tabs"

function Page({ allow = true, asked }: { allow?: boolean; asked?: (t: string) => void }) {
  const [tab, setTab] = useState("one")
  return (
    <Tabs
      value={tab}
      onValueChange={(t) => {
        asked?.(t as string)
        if (allow) setTab(t as string)
      }}
    >
      <TabsList>
        <TabsTrigger value="one">One</TabsTrigger>
        <TabsTrigger value="two">Two</TabsTrigger>
        <TabsTrigger value="three">Three</TabsTrigger>
      </TabsList>
      <TabsContent value="one">First</TabsContent>
      <TabsContent value="two">Second</TabsContent>
      <TabsContent value="three">Third</TabsContent>
    </Tabs>
  )
}

const tab = (name: string) => screen.getByRole("tab", { name })

describe("Tabs", () => {
  it("arrow keys select as they move", async () => {
    render(<Page />)
    await userEvent.click(tab("One"))
    await userEvent.keyboard("{ArrowRight}")
    expect(tab("Two")).toHaveFocus()
    expect(tab("Two")).toHaveAttribute("aria-selected", "true")
    await userEvent.keyboard("{End}")
    expect(tab("Three")).toHaveAttribute("aria-selected", "true")
  })

  it("a click selects", async () => {
    render(<Page />)
    await userEvent.click(tab("Three"))
    expect(tab("Three")).toHaveAttribute("aria-selected", "true")
    expect(screen.getByText("Third")).toBeVisible()
  })

  it("focus from code does not select", async () => {
    render(<Page />)
    act(() => tab("Two").focus())
    expect(tab("Two")).toHaveFocus()
    expect(tab("One")).toHaveAttribute("aria-selected", "true")
    // Nor does a key pressed earlier: the keyboard only counts for the focus it moves itself.
    await userEvent.keyboard("{Shift}")
    await new Promise((r) => setTimeout(r, 0))
    act(() => tab("Three").focus())
    expect(tab("One")).toHaveAttribute("aria-selected", "true")
  })

  it("a refused change is not asked again when focus comes back", async () => {
    // The owner says no (a LeaveGuard's "Stay"), then focus returns to the tab, as a closing dialog does it.
    const asked = vi.fn()
    render(<Page allow={false} asked={asked} />)
    await userEvent.click(tab("Two"))
    expect(asked).toHaveBeenCalledTimes(1)
    act(() => (document.activeElement as HTMLElement | null)?.blur())
    act(() => tab("Two").focus())
    expect(asked).toHaveBeenCalledTimes(1)
    expect(tab("One")).toHaveAttribute("aria-selected", "true")
  })
})
