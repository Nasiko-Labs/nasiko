// The synthetic CP's sign-in and sign-out, against auth/login.rs's contract: a cookie session, ended by logout.
import { describe, expect, it } from "vitest";
import { createWorld, respond } from "../synthetic";

const url = (path: string) => new URL(path, "http://127.0.0.1:3703");
const cookie = (value: string) => new Headers({ cookie: `access_token=${value}` });

describe("synthetic sign-in and sign-out", () => {
  it("signs a member in with an HttpOnly cookie and refuses anyone else", async () => {
    const world = createWorld();
    const ok = await respond(world, "ws-a", "POST", url("/api/auth/login"), new Headers(), {
      username: "user-1",
    });
    expect(ok.status).toBe(200);
    expect(ok.headers["set-cookie"]).toMatch(/^access_token=user-1@ws-a; .*HttpOnly/);
    const no = await respond(world, "ws-a", "POST", url("/api/auth/login"), new Headers(), {
      username: "user-2",
    });
    expect(no.status).toBe(401);
  });

  it("logout ends the session; a new sign-in starts another", async () => {
    const world = createWorld();
    const me = () =>
      respond(world, "ws-a", "GET", url("/api/me"), cookie("user-1@ws-a"), undefined);
    expect((await me()).status).toBe(200);
    const out = await respond(
      world,
      "ws-a",
      "POST",
      url("/api/auth/logout"),
      cookie("user-1@ws-a"),
      undefined,
    );
    expect(out.status).toBe(204);
    expect((await me()).status).toBe(401);
    await respond(world, "ws-a", "POST", url("/api/auth/login"), new Headers(), {
      username: "user-1",
    });
    expect((await me()).status).toBe(200);
  });
});
