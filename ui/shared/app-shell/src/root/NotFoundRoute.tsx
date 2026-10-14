// Ported from nasiko-ui-lab@1b99bdda common/src/routes/__root.tsx (`notFoundComponent`). Local edits: the router's
// not-found route renders the lab's `NotFound` (app/shell/NotFound.tsx) outside the shell, as the lab's root route
// did; the destination's earlier placeholder page is replaced;
// loaded through `LazyNotFound` (its own chunk, as the lab's on-demand import).
import { LazyNotFound } from "../shell/LazyNotFound";

export function NotFoundRoute() {
  return <LazyNotFound />;
}
