// No lab counterpart: the destination's form of the TanStack Router calls Chat made (`useNavigate`, `useRouter`'s
// `state.location.href`, `<Link to="/chat/$sessionId" params search>`). Local edits: n/a (new file).
/**
 * Chat's navigation over the navigation port. `chatHref` builds the lab's `/chat` and `/chat/$sessionId` links (the
 * id encoded as TanStack encoded `$sessionId`, the search through `hrefFor`, which drops undefined keys);
 * `useChatNav` pushes or replaces without scrolling (the lab's `navigate({ to, params, search, replace })`) and reads
 * the URL of the moment (`currentHref`, the lab's `router.state.location.href`) for callbacks that run after an await.
 */
"use client";

import { usePathname, useRouter, useSearchParams } from "@nasiko/app-core/navigation";
import { useLayoutEffect, useRef } from "react";
import { hrefFor } from "@nasiko/app-core/url-state";

/** `/chat` or `/chat/<id>` with `search` (undefined keys dropped). */
export function chatHref(sessionId: string | undefined, search: Record<string, unknown> = {}) {
  return hrefFor(sessionId ? `/chat/${encodeURIComponent(sessionId)}` : "/chat", search);
}

export function useChatNav() {
  const router = useRouter();
  const pathname = usePathname();
  const query = useSearchParams().toString();
  const href = query ? `${pathname}?${query}` : pathname;
  const current = useRef(href);
  // Committed with the render that shows it, so a callback after an await compares against what is on screen.
  useLayoutEffect(() => {
    current.current = href;
  }, [href]);
  return {
    go(to: string, opts: { replace?: boolean } = {}) {
      if (opts.replace) router.replace(to, { scroll: false });
      else router.push(to, { scroll: false });
    },
    /** The URL on screen (for render). */
    href,
    currentHref: () => current.current,
  };
}
