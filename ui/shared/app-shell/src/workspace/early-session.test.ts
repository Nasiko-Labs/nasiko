// Destination test: the early session request starts only where the workspace layout will ask anyway, and is taken once.
import { createMemoryHistory } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({ mode: "live", signedOut: false }));
const fetchMe = vi.hoisted(() =>
  vi.fn(() => Promise.resolve({ sub: "u1", username: "u1", is_superuser: false })),
);
vi.mock("@nasiko/app-core/env", () => ({
  env: {
    get mode() {
      return state.mode;
    },
  },
}));
vi.mock("@nasiko/app-core/session", () => ({ isSignedOutLocally: () => state.signedOut }));
vi.mock("@nasiko/data-access/product", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  fetchMe,
}));

const { createWorkspaceRouter } = await import("../router");
const { startEarlySession, takeEarlySession } = await import("./early-session");

const routes = [
  { path: "/login", public: true, load: () => Promise.resolve({ default: () => null }) },
  { path: "/agents", load: () => Promise.resolve({ default: () => null }) },
  { path: "/settings/password", redirect: "/account/password" },
];
const routerAt = (url: string) =>
  createWorkspaceRouter(routes, createMemoryHistory({ initialEntries: [url] }));

describe("early session request", () => {
  beforeEach(() => {
    state.mode = "live";
    state.signedOut = false;
    fetchMe.mockClear();
  });
  afterEach(() => {
    void takeEarlySession();
  });

  it("starts GET /api/me for a guarded workspace route, and hands it over once", async () => {
    startEarlySession(routerAt("/agents?q=a"));
    expect(fetchMe).toHaveBeenCalledTimes(1);
    expect(fetchMe).toHaveBeenCalledWith({ signal: expect.any(AbortSignal) });
    const first = takeEarlySession();
    expect(first).not.toBeNull();
    await expect(first).resolves.toMatchObject({ sub: "u1" });
    expect(takeEarlySession()).toBeNull();
  });

  it("does nothing on a public route or a redirect", () => {
    startEarlySession(routerAt("/login?redirect=/agents"));
    startEarlySession(routerAt("/settings/password"));
    expect(fetchMe).not.toHaveBeenCalled();
    expect(takeEarlySession()).toBeNull();
  });

  it("does nothing after a local sign-out, or in mock mode", () => {
    state.signedOut = true;
    startEarlySession(routerAt("/agents"));
    state.signedOut = false;
    state.mode = "mock";
    startEarlySession(routerAt("/agents"));
    expect(fetchMe).not.toHaveBeenCalled();
  });

  it("a session refresh before the layout takes it aborts and drops the early request", () => {
    startEarlySession(routerAt("/agents"));
    const call = fetchMe.mock.calls[0] as unknown as [{ signal: AbortSignal }];
    window.dispatchEvent(new Event("nasiko:session-refresh"));
    expect(call[0].signal.aborted).toBe(true);
    expect(takeEarlySession()).toBeNull();
  });

  it("once taken, a later refresh leaves the taken request alone", () => {
    startEarlySession(routerAt("/agents"));
    const call = fetchMe.mock.calls[0] as unknown as [{ signal: AbortSignal }];
    expect(takeEarlySession()).not.toBeNull();
    window.dispatchEvent(new Event("nasiko:session-refresh"));
    expect(call[0].signal.aborted).toBe(false);
  });

  it("a request nobody takes cannot surface as an unhandled rejection", async () => {
    fetchMe.mockImplementationOnce(() => Promise.reject(new Error("offline")));
    startEarlySession(routerAt("/agents"));
    const request = takeEarlySession();
    await expect(request).rejects.toThrow("offline");
  });
});
