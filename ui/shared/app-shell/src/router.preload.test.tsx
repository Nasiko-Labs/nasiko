// Destination test: a link's intent (navigation port `prefetch`) reaches the route's lazy chunk through
// `router.preloadRoute`, so the click renders without a Suspense fallback; and a preload is code only.
import { createMemoryHistory, type AnyRouter } from "@tanstack/react-router";
import { describe, expect, it, vi } from "vitest";
import { createWorkspaceRouter } from "./router";

describe("route preload", () => {
  it("preloadRoute({ to }) loads the matched route's code and nothing else", async () => {
    const page = vi.fn(() => Promise.resolve({ default: () => null }));
    const other = vi.fn(() => Promise.resolve({ default: () => null }));
    // As the navigation port holds it (router-provider.tsx): any path string, not this tree's literal types.
    const router: AnyRouter = createWorkspaceRouter(
      [
        { path: "/agents", load: page },
        { path: "/flows", load: other },
      ],
      createMemoryHistory({ initialEntries: ["/flows"] }),
    );
    await router.preloadRoute({ to: "/agents" });
    expect(page).toHaveBeenCalledTimes(1);
    expect(other).not.toHaveBeenCalled();
  });

  it("a preload fetches code only: no request leaves the page, and nothing mounts", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    const rendered = vi.fn(() => null);
    const router: AnyRouter = createWorkspaceRouter(
      [{ path: "/agents", load: () => Promise.resolve({ default: rendered }) }],
      createMemoryHistory({ initialEntries: ["/login"] }),
    );
    await router.preloadRoute({ to: "/agents" });
    // No data request (routes have no loaders; the session bootstrap runs only when the layout mounts), and the route's
    // component did not render: nothing can read or keep another workspace's data during a preload.
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(rendered).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });
});
