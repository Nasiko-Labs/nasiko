// Ported from nasiko-ui-lab@1b99bdda common/src/components/shared/panel.test.tsx. Local edits: the lab's `ApiError`
// class is replaced by plain objects with the same fields (`status`, `path`, `serverMessage`), which `PanelError` reads
// structurally; the third case (the observability feature's `ErrorState`) is not ported, as that feature is not here.
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { PanelError } from "./panel";

describe('403 is "No access", never a retryable failure', () => {
  it("PanelError says who to ask and offers no Retry", () => {
    render(
      <PanelError
        error={{ status: 403, serverMessage: "requires admin role", path: "/api/x" }}
        onRetry={vi.fn()}
        what="spend"
      />,
    );
    expect(screen.getByText("No access to spend")).toBeInTheDocument();
    expect(screen.getByText(/requires admin role/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /retry/i })).toBeNull();
  });

  it("PanelError keeps Retry for a server failure", () => {
    render(
      <PanelError
        error={{ status: 500, serverMessage: null, path: "/api/x" }}
        onRetry={vi.fn()}
        what="spend"
      />,
    );
    expect(screen.getByText("Couldn't load spend")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });
});
