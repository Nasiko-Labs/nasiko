import { QueryClient } from "@tanstack/query-core";
import { useQuery } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { useEffect, useRef } from "react";
import { describe, expect, it } from "vitest";
import { ProductQueryProvider, useProductKey, useProductScope } from "./react";

function Probe({ onMount }: { onMount?: () => void }) {
  const scope = useProductScope();
  const key = useProductKey("tokenops", "dashboard", { range: "7d" });
  const mounted = useRef(false);
  useEffect(() => {
    if (!mounted.current) {
      mounted.current = true;
      onMount?.();
    }
  }, [onMount]);
  return (
    <output data-testid="probe">
      {scope.workspaceId}:{scope.epoch}:{JSON.stringify(key)}
    </output>
  );
}

function Rows() {
  const key = useProductKey("rows");
  const q = useQuery({ queryKey: key, queryFn: () => Promise.resolve(`rows of ${key[2]}`) });
  return <output data-testid="rows">{q.data ?? "loading"}</output>;
}

describe("ProductQueryProvider", () => {
  it("provides the scope and builds scoped keys", () => {
    const qc = new QueryClient();
    render(
      <ProductQueryProvider scope={{ workspaceId: "ws-a", epoch: 2 }} queryClient={qc}>
        <Probe />
      </ProductQueryProvider>,
    );
    expect(screen.getByTestId("probe").textContent).toBe(
      `ws-a:2:${JSON.stringify(["nasiko", "product", "ws-a", 2, "tokenops", "dashboard", { range: "7d" }])}`,
    );
  });

  it("useProductScope throws outside the provider", () => {
    const qc = new QueryClient();
    expect(() =>
      render(
        <ProductQueryProvider scope={{ workspaceId: "ws-a", epoch: 0 }} queryClient={qc}>
          <span />
        </ProductQueryProvider>,
      ),
    ).not.toThrow();
    expect(() => render(<Probe />)).toThrow(/no ProductQueryProvider/);
  });

  it("a scope change remounts the subtree, so no observer carries the old scope's data", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    let mounts = 0;
    const count = () => {
      mounts++;
    };
    const view = render(
      <ProductQueryProvider scope={{ workspaceId: "ws-a", epoch: 0 }} queryClient={qc}>
        <Probe onMount={count} />
        <Rows />
      </ProductQueryProvider>,
    );
    expect(await screen.findByText("rows of ws-a")).toBeInTheDocument();
    expect(mounts).toBe(1);
    // Same scope, new object: no remount.
    view.rerender(
      <ProductQueryProvider scope={{ workspaceId: "ws-a", epoch: 0 }} queryClient={qc}>
        <Probe onMount={count} />
        <Rows />
      </ProductQueryProvider>,
    );
    expect(mounts).toBe(1);
    view.rerender(
      <ProductQueryProvider scope={{ workspaceId: "ws-b", epoch: 1 }} queryClient={qc}>
        <Probe onMount={count} />
        <Rows />
      </ProductQueryProvider>,
    );
    expect(mounts).toBe(2);
    // The fresh observer starts from nothing, never from ws-a's rows.
    expect(screen.getByTestId("rows").textContent).toBe("loading");
    expect(await screen.findByText("rows of ws-b")).toBeInTheDocument();
  });
});
