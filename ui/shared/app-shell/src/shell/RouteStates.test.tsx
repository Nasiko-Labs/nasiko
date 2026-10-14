// Ported from nasiko-ui-lab@1b99bdda common/src/app/shell/RouteStates.test.tsx. Local edits: `RouteError` is
// rendered with the error-boundary props (`error`, `reset`) through a small boundary of its own instead of a
// TanStack router with `defaultErrorComponent`; `ApiError` is `ProductApiError`; the query client provider and the
// navigation-port test harness (for `router.refresh`) wrap it; Prettier formatting.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Component, type ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { ProductApiError } from "@nasiko/data-access/product";
import { renderWithRouter } from "@nasiko/testing/router";
import { RouteError } from "./RouteStates";

/** The route error-boundary contract: the thrown value and a reset that re-renders the segment. */
class Boundary extends Component<{ children: ReactNode }, { error: unknown; attempt: number }> {
  override state = { error: null as unknown, attempt: 0 };
  static getDerivedStateFromError(error: unknown) {
    return { error };
  }
  override render() {
    if (this.state.error !== null)
      return (
        <RouteError
          error={this.state.error}
          reset={() => {
            this.setState((s) => ({ error: null, attempt: s.attempt + 1 }));
          }}
        />
      );
    return this.props.children;
  }
}

function renderThrowing(fail: () => unknown) {
  const Page = () => <p>{String(fail())}</p>;
  renderWithRouter(
    <QueryClientProvider client={new QueryClient()}>
      <Boundary>
        <Page />
      </Boundary>
    </QueryClientProvider>,
    { url: "/" },
  );
}

describe("RouteError (the route default)", () => {
  it("turns a render crash into a card with Try again, which re-renders the route", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    let broken = true;
    renderThrowing(() => {
      if (broken) throw new Error("boom");
      return "fine";
    });
    expect(await screen.findByText("This page couldn't load")).toBeInTheDocument();
    broken = false;
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByText("fine")).toBeInTheDocument();
    vi.restoreAllMocks();
  });

  it("renders a 403 as No access, with nothing to retry", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    renderThrowing(() => {
      throw new ProductApiError(403, "requires admin role", "/api/users", "m");
    });
    expect(await screen.findByText("You don't have access to this page")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    vi.restoreAllMocks();
  });
});
