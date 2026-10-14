import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { DataTable, VirtualDataTable } from "../table";
import { Table, TableBody, TableCaption, TableCell, TableRow } from "./table";

/**
 * A table wider than its container scrolls sideways, so the scroll box must be reachable and named for keyboard
 * and screen-reader users (axe `scrollable-region-focusable`). jsdom has no layout: widths are stubbed so only the
 * Table primitive's container (or, virtualised, the outer scroll box) reports overflow.
 */
const realScroll = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollWidth");
const realClient = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");

function stubWidths(overflowing: boolean) {
  Object.defineProperty(HTMLElement.prototype, "scrollWidth", {
    configurable: true,
    get(this: HTMLElement) {
      return this.dataset.slot === "table-container" && overflowing ? 900 : 300;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get: () => 300,
  });
}

afterEach(() => {
  if (realScroll) Object.defineProperty(HTMLElement.prototype, "scrollWidth", realScroll);
  if (realClient) Object.defineProperty(HTMLElement.prototype, "clientWidth", realClient);
});

function Simple(props: { "aria-label"?: string; scrollLabel?: string }) {
  return (
    <Table {...props}>
      <TableCaption>Spend by agent</TableCaption>
      <TableBody>
        <TableRow>
          <TableCell>agent-1</TableCell>
        </TableRow>
      </TableBody>
    </Table>
  );
}

describe("Table scroll region", () => {
  it("adds no tab stop or landmark while the table fits", () => {
    stubWidths(false);
    const { container } = render(<Simple aria-label="Spend" />);
    const box = container.querySelector("[data-slot=table-container]");
    expect(box).not.toHaveAttribute("tabindex");
    expect(box).not.toHaveAttribute("role");
    expect(screen.queryByRole("region")).toBeNull();
  });

  describe("when the table overflows", () => {
    beforeEach(() => {
      stubWidths(true);
    });

    it("is a focusable region named by scrollLabel, with the lab's focus ring", () => {
      render(<Simple aria-label="Spend" scrollLabel="Spend by agent" />);
      const region = screen.getByRole("region", { name: "Spend by agent" });
      expect(region).toHaveAttribute("tabindex", "0");
      expect(region).toHaveClass(
        "focus-visible:ring-2",
        "focus-visible:ring-ring",
        "focus-visible:ring-offset-2",
        "focus-visible:ring-offset-background",
        "focus-visible:outline-none",
      );
      region.focus();
      expect(region).toHaveFocus();
    });

    it("falls back to the table's aria-label", () => {
      render(<Simple aria-label="Spend" />);
      expect(screen.getByRole("region", { name: "Spend" })).toHaveAttribute("tabindex", "0");
    });

    it("DataTable names the region with its caption", () => {
      render(
        <DataTable
          caption="Table q1, 2 rows"
          columns={[{ id: "a", header: "Agent", accessorFn: (r: { a: string }) => r.a }]}
          page={{ rows: [{ a: "x" }, { a: "y" }], offset: 0, limit: 25, total: 2 }}
        />,
      );
      const region = screen.getByRole("region", { name: "Table q1, 2 rows" });
      expect(region).toHaveAttribute("tabindex", "0");
      expect(region).toContainElement(screen.getByRole("table"));
    });
  });

  it("VirtualDataTable's virtualised scroll box is a focusable region named by its label", () => {
    stubWidths(false);
    const data = Array.from({ length: 150 }, (_, i) => ({ id: `r${i}` }));
    render(
      <VirtualDataTable
        label="Attribution"
        columns={[{ id: "id", header: "Id", accessorFn: (r: { id: string }) => r.id }]}
        data={data}
        getRowId={(r) => r.id}
      />,
    );
    const region = screen.getByRole("region", { name: "Attribution" });
    expect(region).toHaveAttribute("tabindex", "0");
    region.focus();
    expect(region).toHaveFocus();
  });
});
