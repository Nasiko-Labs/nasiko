// Ported from nasiko-ui-lab@1b99bdda common/src/app/mount.tsx (the worker start and its failure banner).
// Local edits: lazy-loaded mock-only gate that withholds children until the worker runs;
// a class-based React failure banner; edition handlers load before the worker and stack first.
"use client";

import { useEffect, useState, type ReactNode } from "react";
import { startMocks } from "./browser";
import type { EditionHandlers } from "./handlers";

type Boot = "pending" | "ready" | "failed";

/** One start per page load: React's strict-mode double effect and a remount share it. */
let boot: Promise<void> | undefined;
function startOnce(loadEditionHandlers?: () => Promise<EditionHandlers>): Promise<void> {
  boot ??= (async () => {
    const editionHandlers = loadEditionHandlers ? await loadEditionHandlers() : undefined;
    await startMocks({
      envAnchor:
        typeof import.meta.env.VITE_NASIKO_SEED_ANCHOR === "string"
          ? import.meta.env.VITE_NASIKO_SEED_ANCHOR
          : undefined,
      editionHandlers,
    });
  })();
  return boot;
}

export function MockGate({
  children,
  loadEditionHandlers,
}: {
  children: ReactNode;
  loadEditionHandlers?: (() => Promise<EditionHandlers>) | undefined;
}) {
  const [state, setState] = useState<Boot>("pending");
  useEffect(() => {
    let live = true;
    startOnce(loadEditionHandlers).then(
      () => {
        if (live) setState("ready");
      },
      (err: unknown) => {
        console.error(
          "[nasiko] Mock mode, but the MSW worker failed to start. Run `pnpm exec msw init oss/app/public --save=false` and reload.",
          err,
        );
        if (live) setState("failed");
      },
    );
    return () => {
      live = false;
    };
  }, [loadEditionHandlers]);
  // Nothing renders until the worker answers requests: a page that fetched before it started would see the
  // real (absent) server, not the seed.
  if (state === "pending") return null;
  return (
    <>
      {state === "failed" && (
        <div role="alert" className="border-b px-4 py-2 text-sm">
          Mock mode, but the MSW worker failed to start. Run{" "}
          <code>pnpm exec msw init oss/app/public --save=false</code> and reload.
        </div>
      )}
      {children}
    </>
  );
}
