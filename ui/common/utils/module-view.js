/**
 * The active view of a module page, carried in the URL.
 *
 * A module page (Orchestrator, Agents, …) holds every view of its module in one
 * document and swaps between them in place — the shell and the nested sidebar
 * are never rebuilt. That is deliberately *not* routing: nothing is fetched on
 * a switch, no history entry is pushed, and the back button leaves the page
 * rather than stepping through views.
 *
 * The URL still has to name the view, or a shared link, a reload, or a
 * "back to builds" link could not land anywhere but the default. So the param
 * is read once when the page loads and rewritten with `replaceState` on each
 * switch — the same contract `app-tabs` uses for its `query-param`, kept here
 * so the nav, the shell, and every linking page agree on one spelling.
 */
export const VIEW_PARAM = "view";

/** The view named in the current URL, or null. */
export function viewFromUrl() {
  return new URLSearchParams(location.search).get(VIEW_PARAM);
}

/**
 * Which view a freshly loaded page should show.
 *
 * Only honours the URL when it names a view the page actually has: a stale
 * link or a hand-edited param falls back to the default instead of selecting
 * nothing and rendering an empty page.
 */
export function initialView(views, fallback = views[0]) {
  const wanted = viewFromUrl();
  return views.includes(wanted) ? wanted : fallback;
}

/** Point the URL at `view` without touching history depth. */
export function syncView(view) {
  const url = new URL(location.href);
  url.searchParams.set(VIEW_PARAM, view);
  history.replaceState(null, "", `${url.pathname}${url.search}${url.hash}`);
}
