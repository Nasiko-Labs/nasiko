// Ported from the legacy ui/ on development (3aac84f66) common/src/app/shell/TopBar.tsx. Local edits: links are the
// navigation port's `Link` (`href` for TanStack's `to`); `BreadcrumbLink asChild` -> `render={…}` (Base UI
// useRender); primitives and `useMediaQuery` come from `@nasiko/ui`; optional props admit `undefined`
// (exactOptionalPropertyTypes); Prettier formatting.
"use client";

import {
  Fragment,
  isValidElement,
  use,
  useLayoutEffect,
  useState,
  type ReactElement,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useMediaQuery } from "@nasiko/ui/lib";
import { cn } from "@nasiko/ui/cn";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbSeparator,
  SidebarTrigger,
} from "@nasiko/ui/primitives";
import Link from "../navigation";
import { copy } from "./copy";
import { TopBarContext, type TopBarSlot } from "./topBarSlot";

/**
 * The shell's bar above every page: the phones' menu button, the breadcrumb, then the page's actions and its
 * Previous/Next at the far right, so the arrows keep one place on every page. Without a page's `TopBar` the
 * breadcrumb is the page's nav group (text) and item, a link once you are below it. `bare` (Overview, New chat) keeps
 * it on phones only, for the menu button.
 */
export function ShellTopBar({
  item,
  pathname,
  bare = false,
  children,
}: {
  item: TopBarSlot["item"];
  pathname: string;
  bare?: boolean | undefined;
  /** The page area: it reads the slot. */
  children: ReactNode;
}) {
  const [crumbs, setCrumbs] = useState<HTMLElement | null>(null);
  const [actions, setActions] = useState<HTMLElement | null>(null);
  const [stepper, setStepper] = useState<HTMLElement | null>(null);
  const [claims, setClaims] = useState(0);
  return (
    <TopBarContext value={{ crumbs, actions, stepper, claims, setClaims, item }}>
      {/* `@container`: a page's bar contents size by the bar (Chat's chip and totals did by the pane). */}
      <header
        data-slot="topbar"
        className={cn(
          "@container sticky top-0 z-30 flex h-(--topbar-h) shrink-0 items-center gap-2 border-b border-border bg-background px-2 md:px-4",
          bare && "md:hidden",
        )}
      >
        <SidebarTrigger aria-label={copy.openMenu} className="size-11 md:hidden" />
        <div className="flex min-w-0 flex-1 items-center gap-2">
          {claims === 0 && item ? (
            <Crumbs
              group={item.group}
              items={[pathname === item.to ? item.label : <Link href={item.to}>{item.label}</Link>]}
            />
          ) : null}
          <div ref={setCrumbs} className="contents" />
        </div>
        <div ref={setActions} className="flex shrink-0 items-center gap-1.5 empty:hidden" />
        <div
          ref={setStepper}
          className="flex shrink-0 items-center border-l border-border pl-2 empty:hidden"
        />
      </header>
      {children}
    </TopBarContext>
  );
}

/**
 * A breadcrumb: the nav group first, as text (a group is a heading, not a page); then every item but the last is a
 * link (pass a port `Link`); the last is the current page, plain text (or, when it isn't text, drawn as given:
 * Chat's title with its rename button).
 */
function Crumbs({ group, items }: { group?: string | null | undefined; items: ReactNode[] }) {
  const last = items.length - 1;
  return (
    <Breadcrumb aria-label="Breadcrumb" className="min-w-0">
      <BreadcrumbList className="flex-nowrap gap-1.5 sm:gap-1.5">
        {group ? (
          <>
            <BreadcrumbItem className="shrink-0">{group}</BreadcrumbItem>
            <BreadcrumbSeparator />
          </>
        ) : null}
        {items.map((it, i) => (
          // A breadcrumb's levels are positional, never reordered.
          <Fragment key={i}>
            {i > 0 ? <BreadcrumbSeparator /> : null}
            {i < last ? (
              <BreadcrumbItem className="shrink-0">
                {isValidElement(it) ? (
                  <BreadcrumbLink
                    render={it as ReactElement<Record<string, unknown>>}
                    className="underline-offset-2 hover:underline"
                  />
                ) : (
                  it
                )}
              </BreadcrumbItem>
            ) : (
              <BreadcrumbItem className="min-w-0">
                {typeof it === "string" || typeof it === "number" ? (
                  // Plain text, not shadcn's BreadcrumbPage (role="link" on a non-link).
                  <span aria-current="page" className="truncate font-medium text-foreground">
                    {it}
                  </span>
                ) : (
                  it
                )}
              </BreadcrumbItem>
            )}
          </Fragment>
        ))}
      </BreadcrumbList>
    </Breadcrumb>
  );
}

/**
 * A page's part of the top bar. The breadcrumb is the nav group, `root` (default: the page's nav item, as a link),
 * then `trail`, the levels below it ("Settings › Secrets", "Agents › Builds › 5eed…"): every level but the last a
 * port `Link`. Without `root` or `trail` the shell's own breadcrumb stays. `actions` sit at the right, `stepper` (a
 * `PrevNext`) rightmost. Render one per page, in every state the page has (loading, error), so the arrows never
 * move. Below 640 px the actions render here, in the page, so the bar keeps room for the breadcrumb and the arrows;
 * `pinActions` keeps a page's few small ones (Chat's) in the bar.
 */
export function TopBar({
  root,
  trail = [],
  actions,
  stepper,
  pinActions = false,
}: {
  root?: ReactNode;
  trail?: ReactNode[] | undefined;
  actions?: ReactNode;
  stepper?: ReactNode;
  pinActions?: boolean | undefined;
}) {
  const slot = use(TopBarContext);
  const { setClaims, item } = slot;
  const wide = useMediaQuery("(min-width: 640px)");
  const inBar = wide || pinActions;
  const head = root ?? (item ? <Link href={item.to}>{item.label}</Link> : null);
  const claims = root !== undefined || trail.length > 0;
  useLayoutEffect(() => {
    if (!claims) return;
    setClaims((n) => n + 1);
    return () => {
      setClaims((n) => n - 1);
    };
  }, [claims, setClaims]);
  // The group follows the page's nav item (Account settings has none, so no group either).
  const own = claims ? (
    <Crumbs group={item?.group} items={head ? [head, ...trail] : trail} />
  ) : null;
  return (
    <>
      {own && slot.crumbs ? createPortal(own, slot.crumbs) : null}
      {actions && slot.actions && inBar ? createPortal(actions, slot.actions) : null}
      {actions && !inBar ? (
        <div className="flex flex-wrap items-center gap-2">{actions}</div>
      ) : null}
      {stepper && slot.stepper ? createPortal(stepper, slot.stepper) : null}
    </>
  );
}
