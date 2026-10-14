// Destination test: navigation on the app's own router (createWorkspaceRouter + TanStack history), not a harness.
// Deep links, old-link redirects, not-found, and the history index the router owns. The workspace layout (session
// bootstrap, providers) is stubbed: these tests are about routing; the layout has its own tests.
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createBrowserHistory, createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { use, type ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { useParams, useRouter, useSearchParams, useUrlHash } from "@nasiko/app-core/navigation";
import { historyIndex } from "@nasiko/app-core/url-state";

vi.mock("./workspace/WorkspaceLayout", () => ({
  WorkspaceLayout: ({ children }: { children: ReactNode }) => (
    <div>
      <nav aria-label="Workspace">Persistent workspace navigation</nav>
      {children}
    </div>
  ),
}));

const { createWorkspaceRouter } = await import("./router");

function Agent() {
  const { agentId } = useParams<{ agentId: string }>();
  const search = useSearchParams();
  const hash = useUrlHash();
  const router = useRouter();
  return (
    <main>
      <h1>{`agent ${agentId} tab=${search.get("tab") ?? ""} hash=${hash}`}</h1>
      <input aria-label="Draft note" defaultValue="" />
      <button
        onClick={() => {
          router.push(`/agents/${agentId}?tab=activity`);
        }}
      >
        Activity
      </button>
      <button
        onClick={() => {
          router.replace(`/agents/${agentId}?tab=builds`);
        }}
      >
        Builds (replace)
      </button>
    </main>
  );
}
const page = (title: string) => () =>
  Promise.resolve({
    default: () => (
      <main>
        <h1>{title}</h1>
      </main>
    ),
  });
const ROUTES = [
  { path: "/login", public: true, load: page("Sign in") },
  { path: "/agents", load: page("Agents") },
  { path: "/agents/[agentId]", load: () => Promise.resolve({ default: Agent }) },
  { path: "/account/password", load: page("Password") },
  { path: "/settings/password", redirect: "/account/password" },
];

describe("navigation on the app router", () => {
  it("keeps the shell visible and mounted while a cold route chunk is loading", async () => {
    let resolvePage: (page: { default: () => ReactNode }) => void = () => {
      throw new Error("Pending page loader was not initialized");
    };
    const pendingPage = new Promise<{ default: () => ReactNode }>((resolve) => {
      resolvePage = resolve;
    });
    let finishRender = () => {};
    const pendingRender = new Promise<void>((resolve) => {
      finishRender = () => {
        resolve();
      };
    });
    function SlowPage() {
      use(pendingRender);
      return <h1>Slow screen</h1>;
    }
    const load = vi.fn(() => pendingPage);
    const router = createWorkspaceRouter(
      [
        { path: "/agents", load: page("Agents") },
        { path: "/slow", load },
      ],
      createMemoryHistory({ initialEntries: ["/agents"] }),
    );
    render(<RouterProvider router={router} />);
    await screen.findByRole("heading", { name: "Agents" });
    const shell = screen.getByRole("navigation", { name: "Workspace" });
    let navigation: Promise<void>;
    await act(async () => {
      navigation = router.navigate({ to: "/slow" });
    });
    await waitFor(() => {
      expect(load).toHaveBeenCalled();
    });
    await act(async () => {
      resolvePage({ default: SlowPage });
      await navigation;
    });
    // The module is loaded, but its first render suspends: the shell must still stay visible.
    await screen.findByRole("img", { name: "Loading page" });
    expect(shell).toBeVisible();
    expect(screen.getByRole("navigation", { name: "Workspace" })).toBe(shell);
    await act(async () => {
      finishRender();
    });
    await screen.findByRole("heading", { name: "Slow screen" });
    expect(screen.getByRole("navigation", { name: "Workspace" })).toBe(shell);
  });

  it("a deep link renders its route with params, search and fragment from the port", async () => {
    const router = createWorkspaceRouter(
      ROUTES,
      createMemoryHistory({ initialEntries: ["/agents/a1?tab=settings#danger"] }),
    );
    render(<RouterProvider router={router} />);
    expect(
      await screen.findByRole("heading", { name: "agent a1 tab=settings hash=danger" }),
    ).toBeTruthy();
  });

  it("an old link redirects by replacing its entry, so Back does not bounce through it", async () => {
    const history = createMemoryHistory({
      initialEntries: ["/agents", "/settings/password"],
      initialIndex: 1,
    });
    const router = createWorkspaceRouter(ROUTES, history);
    render(<RouterProvider router={router} />);
    await screen.findByRole("heading", { name: "Password" });
    expect(history.location.pathname).toBe("/account/password");
    expect(history.length).toBe(2);
    act(() => {
      history.back();
    });
    await screen.findByRole("heading", { name: "Agents" });
  });

  it("an unknown path renders the not-found page", async () => {
    const router = createWorkspaceRouter(
      ROUTES,
      createMemoryHistory({ initialEntries: ["/weave"] }),
    );
    render(<RouterProvider router={router} />);
    expect(await screen.findByRole("heading", { name: "Page not found" })).toBeTruthy();
  });

  it("does not replay the route entrance or discard drafts on query-only navigation", async () => {
    const router = createWorkspaceRouter(
      ROUTES,
      createMemoryHistory({ initialEntries: ["/agents/a1"] }),
    );
    render(<RouterProvider router={router} />);
    await screen.findByRole("heading", { name: /agent a1 tab= / });
    const input = screen.getByRole("textbox", { name: "Draft note" });
    const entrance = input.closest("[data-route-transition]");
    expect(entrance).not.toBeNull();
    fireEvent.change(input, { target: { value: "Unsaved note" } });
    await act(async () => {
      await router.navigate({ to: "/agents/a1", search: { tab: "activity" } });
    });
    await screen.findByRole("heading", { name: /tab=activity/ });
    expect(screen.getByRole("textbox", { name: "Draft note" })).toBe(input);
    expect(input.closest("[data-route-transition]")).toBe(entrance);
    expect(input).toHaveValue("Unsaved note");
  });

  it("the router numbers history entries: push advances, replace keeps, Back returns", async () => {
    window.history.replaceState(null, "", "/agents/a1");
    const router = createWorkspaceRouter(ROUTES, createBrowserHistory());
    render(<RouterProvider router={router} />);
    await screen.findByRole("heading", { name: /agent a1 tab= / });
    const start = historyIndex();
    act(() => {
      screen.getByRole("button", { name: "Activity" }).click();
    });
    await screen.findByRole("heading", { name: /tab=activity/ });
    expect(historyIndex()).toBe(start + 1);
    act(() => {
      screen.getByRole("button", { name: "Builds (replace)" }).click();
    });
    await screen.findByRole("heading", { name: /tab=builds/ });
    expect(historyIndex()).toBe(start + 1);
    act(() => {
      window.history.back();
    });
    await waitFor(() => {
      expect(historyIndex()).toBe(start);
    });
    expect(await screen.findByRole("heading", { name: /agent a1 tab= / })).toBeTruthy();
  });
});
