// Ported from nasiko-ui-lab@1b99bdda common/src/features/deploy/signOut.deploy.test.ts. Local edits: moved from
// features/deploy to the shell (it tests the shell's `clearLocalState`; a feature may not import @nasiko/app-shell);
// `startUpload`/`follow` take the product scope (`SCOPE`); the R6-order case records chat's `clearDrafts` through
// `vi.mock` instead of `vi.spyOn` on the module (an ES module namespace can't be spied on here); `startWork` waits for
// the upload's XHR to open, so the case never depends on how long the dynamic chunk imports in `clearLocalState`
// take; Prettier formatting.
/**
 * Eng review R3 / task E3: sign-out aborts this tab's uploads and drops followed builds, after chat and drafts (the R6
 * order), so nothing of the previous account finishes or toasts for the next one. Another account's sign-in in another
 * tab reloads this one (sessionSync.ts), which ends uploads with the page, so there is no in-place path to test.
 */
import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";

/** What chat's `clearDrafts` saw of the upload when it ran (the R6 order). */
const order: string[] = [];
vi.mock("@nasiko/features/chat/drafts", async (importOriginal) => {
  const real = await importOriginal<typeof import("@nasiko/features/chat/drafts")>();
  const { uploadFor } = await import("@nasiko/features/deploy/uploads");
  return {
    ...real,
    clearDrafts: (userId?: string) => {
      order.push(`drafts, upload ${uploadFor("u1")?.phase ?? "gone"}`);
      real.clearDrafts(userId);
    },
  };
});
import { clearLocalState } from "./signOut";
import { follow, followedCount, clearFollower } from "@nasiko/features/deploy/follower";
import {
  clearUploads,
  setXhrFactory,
  startUpload,
  uploadFor,
  type XhrFactory,
} from "@nasiko/features/deploy/uploads";

class HeldXhr {
  static last: HeldXhr | null = null;
  aborted = false;
  upload = { onprogress: null as unknown };
  withCredentials = false;
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;
  constructor() {
    HeldXhr.last = this;
  }
  open() {}
  setRequestHeader() {}
  send() {}
  abort() {
    this.aborted = true;
    this.onabort?.();
  }
}

/** The product scope the follower keys its reads under (any fixed scope serves here). */
const SCOPE = { workspaceId: "local", epoch: 0 } as const;

let prev: XhrFactory | null = null;
afterEach(() => {
  if (prev) setXhrFactory(prev);
  clearUploads();
  clearFollower();
});

async function startWork(qc: QueryClient) {
  prev = setXhrFactory(() => new HeldXhr() as unknown as XMLHttpRequest);
  HeldXhr.last = null;
  void startUpload(qc, SCOPE, "u1", new Blob(["z"]), "a.zip", { name: "a" });
  await vi.waitFor(() => {
    expect(HeldXhr.last).not.toBeNull();
  });
  follow(qc, SCOPE, "5eed000b-0000-4000-8000-000000000777", "b");
  expect(uploadFor("u1")?.phase).toBe("uploading");
  expect(followedCount()).toBe(1);
}

describe("deploy work on sign-out (R3)", () => {
  it("sign-out aborts the upload and drops followed builds", async () => {
    const qc = new QueryClient();
    await startWork(qc);
    await clearLocalState(qc, "u1");
    expect(HeldXhr.last?.aborted).toBe(true);
    expect(uploadFor("u1")).toBeNull();
    expect(followedCount()).toBe(0);
  });

  it("clears after chat and drafts (R6 order)", async () => {
    const qc = new QueryClient();
    await startWork(qc);
    order.length = 0;
    await clearLocalState(qc, "u1");
    expect(order).toEqual(["drafts, upload uploading"]);
    expect(uploadFor("u1")).toBeNull();
  });
});
