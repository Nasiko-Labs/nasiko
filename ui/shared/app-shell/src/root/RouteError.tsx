"use client";

import { useEffect } from "react";
import { reportClientError } from "@nasiko/app-core/report-error";
import {
  isChunkLoadError,
  recoverFromChunkError,
  useStaleRecovery,
  type StaleRecovery,
} from "@nasiko/app-core/stale-build";

/** Title and message per outcome: a new version is said only when the server really serves one. */
const CHUNK_COPY: Record<StaleRecovery, { title: string; body: string; reload: boolean }> = {
  checking: { title: "Loading…", body: "Checking why this page could not load.", reload: false },
  reloading: { title: "Nasiko was updated", body: "Loading the new version…", reload: false },
  held: {
    title: "Nasiko was updated",
    body: "Save your changes, then reload to continue.",
    reload: true,
  },
  exhausted: {
    title: "Nasiko was updated",
    body: "This page could not load its code. Reload to continue.",
    reload: true,
  },
  offline: {
    title: "You're offline",
    body: "This page will load when the connection is back.",
    reload: false,
  },
  network: {
    title: "Couldn't reach Nasiko",
    body: "Check the connection, then try again.",
    reload: true,
  },
  failed: { title: "This page couldn't load", body: "Try again in a moment.", reload: true },
};

/**
 * Route error boundary. Shows only the digest (never the message or stack —
 * contract §17.5: no internal exception details in HTML) and offers a reset.
 * A route whose code chunk is gone (a tab older than the running build) gets
 * the stale-build recovery instead (@nasiko/app-core/stale-build).
 */
export function RouteError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  const stale = isChunkLoadError(error);
  const recovery = useStaleRecovery();
  useEffect(() => {
    reportClientError(error);
    if (stale) void recoverFromChunkError();
  }, [error, stale]);
  if (stale) {
    const copy = CHUNK_COPY[recovery ?? "checking"];
    return (
      <section
        role="alert"
        aria-labelledby="err-title"
        className="flex flex-col gap-3"
        data-stale-build={recovery ?? "checking"}
      >
        <h1 id="err-title" className="text-xl font-semibold">
          {copy.title}
        </h1>
        <p>{copy.body}</p>
        {copy.reload && (
          <button
            type="button"
            onClick={() => {
              window.location.reload();
            }}
            className="w-max rounded border px-3 py-1"
          >
            {recovery === "network" || recovery === "failed" ? "Try again" : "Reload"}
          </button>
        )}
      </section>
    );
  }
  return (
    <section role="alert" aria-labelledby="err-title" className="flex flex-col gap-3">
      <h1 id="err-title" className="text-xl font-semibold">
        Something went wrong
      </h1>
      <p>
        The page could not be rendered. Reference:{" "}
        <code data-testid="error-digest">{error.digest ?? "n/a"}</code>
      </p>
      <button type="button" onClick={reset} className="w-max rounded border px-3 py-1">
        Try again
      </button>
    </section>
  );
}
