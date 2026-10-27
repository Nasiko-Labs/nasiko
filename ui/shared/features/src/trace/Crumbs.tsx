/** The session pages' breadcrumb: Sessions, the day it came from (when it did), then this page. */
import { Link } from '@tanstack/react-router'
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbSeparator,
} from '@nasiko/ui/primitives/breadcrumb'
import type { SessionsSearch } from '@nasiko/features/sessions/search'
import { fmtShortDay } from '@nasiko/ui/lib/format'

export function Crumbs({
  back,
  day,
  title,
}: {
  back: Partial<SessionsSearch>
  day?: string
  title: string
}) {
  return (
    <Breadcrumb aria-label="Breadcrumb">
      <BreadcrumbList className="gap-1 sm:gap-1">
        <BreadcrumbItem>
          <BreadcrumbLink
            className="underline-offset-2 hover:underline"
            render={
              <Link
                to="/sessions"
                search={{ ...back, day: undefined, sort: undefined, lane: undefined }}
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
                render={<Link to="/sessions" search={back} />}
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
  )
}
