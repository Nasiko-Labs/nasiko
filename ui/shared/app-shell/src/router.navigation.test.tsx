// Destination test: navigation on the app's own router (createWorkspaceRouter + TanStack history), not a harness.
// Deep links, old-link redirects, not-found, and the history index the router owns. The workspace layout (session
// bootstrap, providers) is stubbed: these tests are about routing; the layout has its own tests.
import { act, render, screen, waitFor } from "@testing-library/react";
import { createBrowserHistory, createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { useParams, useRouter, useSearchParams, useUrlHash } from "@nasiko/app-core/navigation";
import { historyIndex } from "@nasiko/app-core/url-state";

vi.mock("./workspace/WorkspaceLayout", () => ({
  WorkspaceLayout: ({ children }: { children: ReactNode }) => children,
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
