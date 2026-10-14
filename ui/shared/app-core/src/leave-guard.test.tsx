// Destination test (the lab tested LeaveGuard through its pages): the guard on a real TanStack router with memory
// history, as the app runs it (RouterNavigation is the navigation port the features use).
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  createBrowserHistory,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  RouterProvider,
} from "@tanstack/react-router";
import { describe, expect, it } from "vitest";
import { Link, useRouter } from "./navigation";
import { RouterNavigation } from "./router-provider";
import { LeaveGuard, useLeaveBlocker } from "./leave-guard";

function Form({ when, samePath }: { when: boolean; samePath?: boolean }) {
  const router = useRouter();
  return (
    <div>
      <h1>Form</h1>
      <LeaveGuard when={when} samePath={samePath ?? false} />
      <Link href="/other">Other page</Link>
      <Link href="/form?section=b">Section B</Link>
      <Link href="/login">Sign in</Link>
      <button
        onClick={() => {
          router.push("/other");
        }}
      >
        Push
      </button>
      <button
        onClick={() => {
          router.push("/other", { ignoreBlocker: true });
        }}
      >
        Push past the guard
      </button>
    </div>
  );
}

function setup({
  when = true,
  samePath,
  start = "/form",
}: { when?: boolean; samePath?: boolean; start?: string } = {}) {
  const root = createRootRoute({
    component: () => (
      <RouterNavigation>
        <Outlet />
      </RouterNavigation>
    ),
  });
  const routes = [
    createRoute({
      getParentRoute: () => root,
      path: "/form",
      component: () => <Form when={when} {...(samePath === undefined ? {} : { samePath })} />,
    }),
    createRoute({ getParentRoute: () => root, path: "/other", component: () => <h1>Other</h1> }),
    createRoute({ getParentRoute: () => root, path: "/login", component: () => <h1>Login</h1> }),
    createRoute({ getParentRoute: () => root, path: "/start", component: () => <h1>Start</h1> }),
  ];
  const history = createMemoryHistory({ initialEntries: [start] });
  const router = createRouter({ routeTree: root.addChildren(routes), history });
  render(<RouterProvider router={router} />);
  return { router, history };
}

const path = (h: ReturnType<typeof createMemoryHistory>) => h.location.pathname + h.location.search;

describe("LeaveGuard (TanStack useBlocker)", () => {
  it("holds an in-app link behind the dialog; Stay keeps the page, Leave goes", async () => {
    const user = userEvent.setup();
    const { history } = setup();
    await screen.findByRole("heading", { name: "Form" });
    await user.click(screen.getByRole("link", { name: "Other page" }));
    expect(await screen.findByRole("alertdialog")).toBeTruthy();
    expect(path(history)).toBe("/form");
    await user.click(screen.getByRole("button", { name: "Stay" }));
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
    expect(screen.getByRole("heading", { name: "Form" })).toBeTruthy();
    await user.click(screen.getByRole("link", { name: "Other page" }));
    await user.click(await screen.findByRole("button", { name: "Leave" }));
    await screen.findByRole("heading", { name: "Other" });
    expect(path(history)).toBe("/other");
  });

  it("holds programmatic navigation, and lets `ignoreBlocker` through", async () => {
    const user = userEvent.setup();
    const { history } = setup();
    await screen.findByRole("heading", { name: "Form" });
    await user.click(screen.getByRole("button", { name: "Push" }));
    expect(await screen.findByRole("alertdialog")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Stay" }));
    await user.click(screen.getByRole("button", { name: "Push past the guard" }));
    await screen.findByRole("heading", { name: "Other" });
    expect(path(history)).toBe("/other");
  });

  // Back/Forward blocking is the browser history's (its popstate handler asks the blockers and reverts the
  // traversal with `history.go(-delta)` when held); memory history has none, so this one runs on jsdom's history.
  it("holds Back: the history traversal waits for the dialog", async () => {
    const user = userEvent.setup();
    // The router writes the entries (each carries its `__TSR_index`, which the popstate handler compares).
    window.history.replaceState(null, "", "/start");
    const history = createBrowserHistory();
    const root = createRootRoute({
      component: () => (
        <RouterNavigation>
          <Outlet />
        </RouterNavigation>
      ),
    });
    const router = createRouter({
      routeTree: root.addChildren([
        createRoute({ getParentRoute: () => root, path: "/form", component: () => <Form when /> }),
        createRoute({
          getParentRoute: () => root,
          path: "/start",
          component: () => <h1>Start</h1>,
        }),
      ]),
      history,
    });
    render(<RouterProvider router={router} />);
    await screen.findByRole("heading", { name: "Start" });
    await act(() => router.navigate({ to: "/form" }));
    await screen.findByRole("heading", { name: "Form" });
    act(() => {
      history.back();
    });
    expect(await screen.findByRole("alertdialog")).toBeTruthy();
    expect(path(history)).toBe("/form");
    await user.click(screen.getByRole("button", { name: "Stay" }));
    expect(screen.getByRole("heading", { name: "Form" })).toBeTruthy();
    // Stay reverts the traversal (`history.go(+1)`, asynchronous): the next Back starts after it has landed.
    await waitFor(() => {
      expect(window.location.pathname).toBe("/form");
    });
    await new Promise((resolve) => setTimeout(resolve, 50));
    act(() => {
      history.back();
    });
    await user.click(await screen.findByRole("button", { name: "Leave" }));
    // jsdom's history traversal is queued, and Leave re-runs it: allow it longer than one second.
    await screen.findByRole("heading", { name: "Start" }, { timeout: 5000 });
    expect(path(history)).toBe("/start");
  });

  // Forward goes through the same popstate handler, with a positive delta: hold it, then let it through on Leave.
  it("holds Forward the same way", async () => {
    const user = userEvent.setup();
    window.history.replaceState(null, "", "/form");
    const history = createBrowserHistory();
    const root = createRootRoute({
      component: () => (
        <RouterNavigation>
          <Outlet />
        </RouterNavigation>
      ),
    });
    const router = createRouter({
      routeTree: root.addChildren([
        createRoute({ getParentRoute: () => root, path: "/form", component: () => <Form when /> }),
        createRoute({
          getParentRoute: () => root,
          path: "/other",
          component: () => <h1>Other</h1>,
        }),
      ]),
      history,
    });
    render(<RouterProvider router={router} />);
    await screen.findByRole("heading", { name: "Form" });
    // Leave the dirty form past its guard, then come Back to it (leaving /other holds nothing).
    await user.click(screen.getByRole("button", { name: "Push past the guard" }));
    await screen.findByRole("heading", { name: "Other" });
    act(() => {
      history.back();
    });
    await screen.findByRole("heading", { name: "Form" }, { timeout: 5000 });
    await new Promise((resolve) => setTimeout(resolve, 50));
    act(() => {
      history.forward();
    });
    expect(await screen.findByRole("alertdialog")).toBeTruthy();
    expect(path(history)).toBe("/form");
    await user.click(screen.getByRole("button", { name: "Leave" }));
    await screen.findByRole("heading", { name: "Other" }, { timeout: 5000 });
    expect(path(history)).toBe("/other");
  });

  it("always lets /login through, and nothing is held while the form is clean", async () => {
    const user = userEvent.setup();
    setup();
    await screen.findByRole("heading", { name: "Form" });
    await user.click(screen.getByRole("link", { name: "Sign in" }));
    await screen.findByRole("heading", { name: "Login" });
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("is inert when `when` is false", async () => {
    const user = userEvent.setup();
    setup({ when: false });
    await screen.findByRole("heading", { name: "Form" });
    await user.click(screen.getByRole("link", { name: "Other page" }));
    await screen.findByRole("heading", { name: "Other" });
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("`samePath` lets a search-only move through and still holds another page", async () => {
    const user = userEvent.setup();
    const { history } = setup({ samePath: true });
    await screen.findByRole("heading", { name: "Form" });
    await user.click(screen.getByRole("link", { name: "Section B" }));
    await waitFor(() => {
      expect(path(history)).toBe("/form?section=b");
    });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await user.click(screen.getByRole("link", { name: "Other page" }));
    expect(await screen.findByRole("alertdialog")).toBeTruthy();
  });

  it("useLeaveBlocker reports the held navigation without a dialog of its own", async () => {
    const user = userEvent.setup();
    function Custom() {
      const blocker = useLeaveBlocker({ when: true });
      return (
        <div>
          <h1>Custom</h1>
          <Link href="/other">Other page</Link>
          {blocker.blocked ? <button onClick={blocker.proceed}>Discard and leave</button> : null}
        </div>
      );
    }
    const root = createRootRoute({
      component: () => (
        <RouterNavigation>
          <Outlet />
        </RouterNavigation>
      ),
    });
    const router = createRouter({
      routeTree: root.addChildren([
        createRoute({ getParentRoute: () => root, path: "/form", component: Custom }),
        createRoute({
          getParentRoute: () => root,
          path: "/other",
          component: () => <h1>Other</h1>,
        }),
      ]),
      history: createMemoryHistory({ initialEntries: ["/form"] }),
    });
    render(<RouterProvider router={router} />);
    await screen.findByRole("heading", { name: "Custom" });
    await user.click(screen.getByRole("link", { name: "Other page" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await user.click(await screen.findByRole("button", { name: "Discard and leave" }));
    await screen.findByRole("heading", { name: "Other" });
  });
});
