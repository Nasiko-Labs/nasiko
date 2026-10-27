// A single-choice ToggleGroup is a WAI-ARIA radio group, as
// the lab's Radix `type="single"` groups were radios; a `multiple` group stays pressed buttons.
import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { useState } from "react"
import { describe, expect, it } from "vitest"
import { ToggleGroup, ToggleGroupItem } from "./toggle-group"

function Range({
  loop = true,
  initial = ["30d"],
}: {
  loop?: boolean
  initial?: string[]
}) {
  const [v, setV] = useState<readonly string[]>(initial)
  return (
    <ToggleGroup
      aria-label="Range"
      value={v}
      onValueChange={setV}
      loopFocus={loop}
    >
      <ToggleGroupItem value="7d">7 days</ToggleGroupItem>
      <ToggleGroupItem value="30d">30 days</ToggleGroupItem>
      <ToggleGroupItem value="90d">90 days</ToggleGroupItem>
    </ToggleGroup>
  )
}

const radio = (name: string) => screen.getByRole("radio", { name })

describe("single-choice ToggleGroup = radio group", () => {
  it("has radiogroup / radio roles with aria-checked, never aria-pressed", () => {
    render(<Range />)
    expect(
      screen.getByRole("radiogroup", { name: "Range" })
    ).toBeInTheDocument()
    expect(radio("30 days")).toHaveAttribute("aria-checked", "true")
    expect(radio("7 days")).toHaveAttribute("aria-checked", "false")
    for (const r of screen.getAllByRole("radio"))
      expect(r).not.toHaveAttribute("aria-pressed")
  })

  it("has one tab stop: the checked item", async () => {
    render(
      <>
        <button>before</button>
        <Range />
      </>
    )
    await userEvent.click(screen.getByRole("button", { name: "before" }))
    await userEvent.tab()
    expect(radio("30 days")).toHaveFocus()
    await userEvent.tab()
    expect(document.body).toHaveFocus()
  })

  it("Arrow keys move focus and select; Home/End jump; wraps by default", async () => {
    render(<Range />)
    radio("30 days").focus()
    await userEvent.keyboard("{ArrowRight}")
    expect(radio("90 days")).toHaveFocus()
    expect(radio("90 days")).toHaveAttribute("aria-checked", "true")
    await userEvent.keyboard("{ArrowRight}")
    expect(radio("7 days")).toHaveAttribute("aria-checked", "true")
    await userEvent.keyboard("{End}")
    expect(radio("90 days")).toHaveAttribute("aria-checked", "true")
    await userEvent.keyboard("{Home}")
    expect(radio("7 days")).toHaveAttribute("aria-checked", "true")
    await userEvent.keyboard("{ArrowLeft}")
    expect(radio("90 days")).toHaveAttribute("aria-checked", "true")
  })

  it("does not wrap with loopFocus={false}", async () => {
    render(<Range loop={false} initial={["90d"]} />)
    radio("90 days").focus()
    await userEvent.keyboard("{ArrowRight}")
    expect(radio("90 days")).toHaveAttribute("aria-checked", "true")
    expect(radio("90 days")).toHaveFocus()
  })

  it("Space on the checked radio keeps it checked (a radio cannot be unchecked)", async () => {
    render(<Range />)
    radio("30 days").focus()
    await userEvent.keyboard(" ")
    expect(radio("30 days")).toHaveAttribute("aria-checked", "true")
  })
})

describe("multiple ToggleGroup = pressed buttons", () => {
  it("keeps toggle-button semantics", () => {
    render(
      <ToggleGroup aria-label="Filters" multiple defaultValue={["a"]}>
        <ToggleGroupItem value="a">A</ToggleGroupItem>
        <ToggleGroupItem value="b">B</ToggleGroupItem>
      </ToggleGroup>
    )
    expect(screen.queryByRole("radio")).toBeNull()
    expect(screen.getByRole("button", { name: "A" })).toHaveAttribute(
      "aria-pressed",
      "true"
    )
  })
})
