"use client";

/** Last-resort boundary when the root layout itself fails. Minimal, redacted. */
export function GlobalError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  return (
    <html lang="en">
      <body>
        <main role="alert">
          <h1>Nasiko is unavailable</h1>
          <p>Reference: {error.digest ?? "n/a"}</p>
          <button type="button" onClick={reset}>
            Retry
          </button>
        </main>
      </body>
    </html>
  );
}
