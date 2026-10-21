// @vitest-environment node
import { describe, expect, it } from "vitest";
import { dueTour, markSeen, NO_PREFS, parsePrefs, TOUR_IDS, tourFor } from "./logic";
import { stepsFor, TOURS } from "./tours";

describe("tourFor", () => {
  it.each([
    ["/", "overview"],
    ["/chat", "chat"],
    ["/chat/abc", "chat"],
    ["/agents", "agents"],
    ["/agents/mine", "agents"],
    ["/agents/5eed-1", "agent"],
    ["/deploy", "deploy"],
    ["/builds", "builds"],
    ["/workflows/runs", "workflows"],
    ["/sessions", "sessions"],
    ["/sessions/5eed-sess", "session"],
    ["/settings/secrets", "settings"],
    ["/builds/abc", "build"],
    ["/flows/abc", "flow"],
    ["/mcp/abc", "mcp-server"],
    ["/workflows/new", "workflow-new"],
    ["/workflows/drafts", "workflows"],
    ["/workflows/abc", "workflow"],
    ["/account/appearance", "account"],
    ["/status", "status"],
  ])("%s → %s", (path, id) => expect(tourFor(path)).toBe(id));

  it.each(["/weave", "/login"])("has none on %s", (path) => expect(tourFor(path)).toBeNull());

  it("falls back to a layer's tours, after the core's", () => {
    const layers = [
      { id: "access", match: /^\/access(\/users)?$/ },
      { id: "never", match: /^\/agents$/ },
    ];
    expect(tourFor("/access/users", layers)).toBe("access");
    expect(tourFor("/agents", layers)).toBe("agents");
    expect(dueTour({ auto: true, seen: ["app"] }, "/access", layers)).toEqual({
      id: "access",
      withApp: false,
    });
  });
});

describe("dueTour", () => {
  const on = { auto: true, seen: [] };
  it("starts nothing while tours are off", () => expect(dueTour(NO_PREFS, "/")).toBeNull());
  it("folds the app tour into the first one, wherever the user is", () => {
    expect(dueTour(on, "/tokenops")).toEqual({ id: "tokenops", withApp: true });
    expect(dueTour(on, "/weave")).toEqual({ id: "app", withApp: true });
  });
  it("then each page's own, once", () => {
    const afterApp = markSeen(markSeen(on, "app"), "tokenops");
    expect(dueTour(afterApp, "/tokenops")).toBeNull();
    expect(dueTour(afterApp, "/sessions")).toEqual({ id: "sessions", withApp: false });
    expect(dueTour(afterApp, "/weave")).toBeNull();
  });
});

describe("parsePrefs", () => {
  it("reads what writePrefs stores", () =>
    expect(parsePrefs('{"auto":true,"seen":["app","chat"]}')).toEqual({
      auto: true,
      seen: ["app", "chat"],
    }));
  it.each([null, "", "not json", "[]", '{"auto":"yes","seen":"app"}'])(
    "reads %j as tours off",
    (raw) => expect(parsePrefs(raw)).toEqual(NO_PREFS),
  );
  it("keeps any id (a layer's too) and drops what isn't one", () =>
    expect(parsePrefs('{"auto":true,"seen":["app","access",3,null]}').seen).toEqual([
      "app",
      "access",
    ]));
});

describe("tours", () => {
  it.each(TOUR_IDS)("%s opens with a centred step, so it never comes out empty", (id) =>
    expect(TOURS[id][0]?.target).toBeUndefined(),
  );
});

describe("stepsFor", () => {
  it("puts the welcome and the sidebar first, the page next and the tour button last", () => {
    const steps = stepsFor(TOURS.overview, true);
    expect(steps[0]).toBe(TOURS.app[0]);
    expect(steps.map((s) => s.target)).toContain("overview-needs");
    expect(steps.at(-1)?.target).toBe("tour-button");
    expect(steps).toHaveLength(TOURS.app.length + TOURS.overview.length);
  });
  it("is the page alone without the app tour", () =>
    expect(stepsFor(TOURS.tokenops, false)).toBe(TOURS.tokenops));
});
