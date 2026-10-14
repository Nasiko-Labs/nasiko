import type { ReactNode } from "react";

/**
 * The foundation and verification-harness pages of every composition root (no product shell): the neutral `<main>` and skip link that used to live in the
 * root layout. The product shell (`(app)`) and the sign-in page bring their own `<main>` landmark, so the root
 * layout renders bare children.
 */
export function FoundationLayout({ children }: { children: ReactNode }) {
  return (
    <>
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-50 focus:rounded focus:bg-background focus:px-3 focus:py-2 focus:outline focus:outline-2"
      >
        Skip to content
      </a>
      <main id="main" className="mx-auto w-full max-w-5xl px-4 py-8">
        {children}
      </main>
    </>
  );
}
