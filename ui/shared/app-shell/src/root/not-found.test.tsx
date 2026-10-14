import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { NotFoundRoute as NotFound } from "./NotFoundRoute";

// The root not-found boundary is the lab's `NotFound` (app/shell/NotFound.tsx), loaded on demand through
// `LazyNotFound` as the lab did: its way back is "Back to Overview".
describe("not-found boundary", () => {
  it("renders a labelled heading and a way back", async () => {
    render(<NotFound />);
    expect(await screen.findByRole("heading", { name: /page not found/i })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /back to overview/i })).toHaveAttribute("href", "/");
    expect(screen.getByRole("link", { name: /go to chat/i })).toHaveAttribute("href", "/chat");
  });
});
