// Ported from the legacy ui/ on development (3aac84f66) common/src/features/trace/Crumbs.tsx (moved out of
// SessionTracePage.tsx for the session ledger, 84c6f11eb). Local edits: as SessionTracePage's port had them: the
// navigation port's `Link` inside `BreadcrumbLink render={…}` (Base UI useRender) with `hrefFor('/sessions', …)`;
// `day` admits `undefined` (exactOptionalPropertyTypes); Prettier formatting.
/** The session pages' breadcrumb: Sessions, the day it came from (when it did), then this page. */
import Link from "@nasiko/app-core/navigation";
import { hrefFor } from "@nasiko/app-core/url-state";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbSeparator,
} from "@nasiko/ui/primitives";
import { fmtShortDay } from "@nasiko/ui/lib";
import type { SessionsSearch } from "../sessions/search";

export function Crumbs({
  back,
  day,
  title,
}: {
  back: Partial<SessionsSearch>;
  day?: string | undefined;
  title: string;
}) {
  return (
    <Breadcrumb aria-label="Breadcrumb">
      <BreadcrumbList className="gap-1 sm:gap-1">
        <BreadcrumbItem>
          <BreadcrumbLink
            className="underline-offset-2 hover:underline"
            render={
              <Link
                href={hrefFor("/sessions", {
                  ...back,
                  day: undefined,
                  sort: undefined,
                  lane: undefined,
                })}
              />
            }
          >
            Sessions
          </BreadcrumbLink>
        </BreadcrumbItem>
        {day ? (
          <>
            <BreadcrumbSeparator />
            <BreadcrumbItem>
              <BreadcrumbLink
                className="underline-offset-2 hover:underline"
                render={<Link href={hrefFor("/sessions", back)} />}
              >
                {fmtShortDay(day)}
              </BreadcrumbLink>
            </BreadcrumbItem>
          </>
        ) : null}
        <BreadcrumbSeparator />
        <BreadcrumbItem className="min-w-0">
          {/* Plain text, not shadcn's BreadcrumbPage (role="link" on a non-link). */}
          <span aria-current="page" className="truncate text-foreground">
            {title}
          </span>
        </BreadcrumbItem>
      </BreadcrumbList>
    </Breadcrumb>
  );
}
