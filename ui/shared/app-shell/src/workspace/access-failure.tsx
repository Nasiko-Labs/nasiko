// Ported from nasiko-ui-lab@1b99bdda common/src/app/shell/RouteStates.tsx (the two `RouteError` cards, rendered by
// the `(app)` layout for a server-side identity failure). Local edits: a server component that picks the card
// by the failure kind (`not-a-member` is the lab's 403 "No access"; `delegation-unavailable` the generic error with
// Try again, which reloads the page since no router reset exists on the server); Prettier formatting.
import { StateCard } from "@nasiko/ui/composites";
import { copy } from "@nasiko/app-core/shell/copy";
import { ReloadButton } from "./reload-button";

export function AccessFailure({ kind }: { kind: "not-a-member" | "delegation-unavailable" }) {
  if (kind === "not-a-member") {
    return (
      <div className="mx-auto mt-10 max-w-lg px-4">
        <StateCard
          tone="warning"
          title={copy.routeError.noAccess}
          fix={copy.routeError.noAccessBody}
        />
      </div>
    );
  }
  return (
    <div className="mx-auto mt-10 max-w-lg px-4">
      <StateCard
        tone="error"
        title={copy.routeError.title}
        fix={copy.routeError.body}
        action={<ReloadButton />}
      />
    </div>
  );
}
