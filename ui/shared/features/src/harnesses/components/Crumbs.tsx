// Ported from nasiko-ui-lab@1b99bdda common/src/features/harnesses/components/Crumbs.tsx. Local edits:
// `BreadcrumbLink asChild` -> `render={<Button …/>}` (Base UI useRender; the label stays the link's child); optional
// props typed `| undefined`; Prettier formatting.
// The top bar (legacy ui/ on development, 3aac84f66: e8379db5a): the drill path is labelled "Levels", so the page
// has one "Breadcrumb" nav.
/**
 * Breadcrumb (plan §3, G8): follows the drill path; the root is "Org" (admins) or
 * "Your units" (multi-root managers); middle items collapse to "…" on narrow screens.
 * Built on shadcn `Breadcrumb`; drill steps are buttons (they set search params, not hrefs).
 */
import { X } from "lucide-react";
import { Fragment, useState } from "react";
import {
  Breadcrumb,
  BreadcrumbEllipsis,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@nasiko/ui/primitives";
import { Button } from "@nasiko/ui/primitives";
import { useMediaQuery } from "@nasiko/ui/lib";
import { NARROW_BREAKPOINT_PX } from "../constants";
import { HarnessLabel } from "./bits";

export interface Crumb {
  label: string;
  /** Undefined for the current page. */
  onClick?: (() => void) | undefined;
}

const crumbKey = (c: Crumb | "ellipsis") => (c === "ellipsis" ? "…" : c.label);

export function Crumbs({
  items,
  harness,
  onClearHarness,
}: {
  items: Crumb[];
  harness?: string | undefined;
  onClearHarness: () => void;
}) {
  const narrow = !useMediaQuery(`(min-width: ${NARROW_BREAKPOINT_PX}px)`, true);
  const [expanded, setExpanded] = useState(false);
  const collapse = narrow && !expanded && items.length > 3;
  const first = items[0];
  const last = items.at(-1);
  const shown: (Crumb | "ellipsis")[] =
    collapse && first && last ? [first, "ellipsis", last] : items;
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Breadcrumb aria-label="Levels">
        <BreadcrumbList className="gap-1 sm:gap-1">
          {shown.map((c, i) => (
            // The path up to this crumb: unique even when two units share a name.
            <Fragment
              key={shown
                .slice(0, i + 1)
                .map(crumbKey)
                .join("/")}
            >
              {i > 0 ? <BreadcrumbSeparator /> : null}
              <BreadcrumbItem>
                {c === "ellipsis" ? (
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    onClick={() => {
                      setExpanded(true);
                    }}
                    aria-label="Show full path"
                  >
                    <BreadcrumbEllipsis className="size-auto" />
                  </Button>
                ) : c.onClick ? (
                  <BreadcrumbLink
                    render={
                      <Button
                        variant="link"
                        onClick={c.onClick}
                        className="h-auto min-h-8 p-0 font-normal text-muted-foreground hover:text-foreground max-sm:min-h-11"
                      />
                    }
                  >
                    {c.label}
                  </BreadcrumbLink>
                ) : (
                  <BreadcrumbPage className="font-medium">{c.label}</BreadcrumbPage>
                )}
              </BreadcrumbItem>
            </Fragment>
          ))}
        </BreadcrumbList>
      </Breadcrumb>
      {harness ? (
        <Button
          variant="outline"
          size="sm"
          onClick={onClearHarness}
          className="rounded-full text-xs font-normal"
          aria-label="Remove harness filter"
        >
          <HarnessLabel id={harness} /> <X className="size-3" aria-hidden />
        </Button>
      ) : null}
    </div>
  );
}
