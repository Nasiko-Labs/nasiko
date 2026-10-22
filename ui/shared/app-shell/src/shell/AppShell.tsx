// Ported from nasiko-ui-lab@1b99bdda common/src/app/shell/AppShell.tsx. Shared navigation port, children slot, admitted
// primitives and browser cookie/width default. Development (3aac84f66, 66a4c64d0): a layer's `split` (the `shellSplit`
// slot) wraps the page area; e8379db5a: one top bar (`ShellTopBar`, @nasiko/app-core/shell/TopBar) above every page
// replaces the phones' slim bar. Development (309a7bd8f).
"use client";

/**
 * The signed-in layout (plans/feat-app-shell.md §4.1): sidebar + page area. Rendered by the `(app)` layout, so
 * /login stays bare. Pages keep their own max width; Chat fills the viewport height.
 */
import { usePathname } from "@nasiko/app-core/navigation";
import { use, useState, useSyncExternalStore, type ComponentType, type ReactNode } from "react";
import { Button, SidebarInset, SidebarProvider } from "@nasiko/ui/primitives";
import { applyNav } from "@nasiko/app-core/edition/edition";
import { EditionContext } from "@nasiko/app-core/edition/edition-context";
import { activeItem, NAV_GROUPS, NAV_ITEMS } from "@nasiko/app-core/shell/nav";
import { ShellTopBar } from "@nasiko/app-core/shell/TopBar";
import { cn } from "@nasiko/ui/cn";
import { StaleBuildNotice } from "./StaleBuildNotice";
import { AppSidebar, type NavBadges } from "./AppSidebar";
import { copy } from "@nasiko/app-core/shell/copy";
import { SidebarPanelContext } from "@nasiko/app-core/shell/panelSlot";
import { widthDefaultOpen, readSidebarCookie } from "@nasiko/app-core/shell/sidebarState";

/** Chat, Settings and Account own their scrolling (their section column stays put): the page fills the viewport (plan §7.1, EN20). */
const FULL_HEIGHT = /^\/(chat|settings|account)(\/|$)/;
/** Pages without the top bar from `md` up (phones keep it for the menu button). */
const BARE = /^\/(chat\/?)?$/;

const noop = () => () => undefined;
/** Fallback for non-browser unit rendering; production reads the actual width before mounting. */
const serverWidthDefault = () => true;

/**
 * `end` and `badges` come from the `(app)` layout, which may import features (the shell never does): Deploy's build
 * toasts and the Builds item's in-progress count.
 */
export function AppShell({
  children,
  end,
  split: Split,
  badges,
  hidden,
  initialOpen,
}: {
  children?: ReactNode;
  end?: ReactNode;
  /** A layer's split around the page (the `shellSplit` slot; EE: its assistant panel). */
  split?: ComponentType<{ path: string; children: ReactNode }> | null | undefined;
  badges?: NavBadges | undefined;
  /** Nav paths to leave out (onboarding hides Home while its guide stands in for it). */
  hidden?: readonly string[] | undefined;
  /** Optional browser cookie snapshot; undefined falls back to the current cookie and width. */
  initialOpen?: boolean | undefined;
}) {
  const pathname = usePathname();
  const fullHeight = FULL_HEIGHT.test(pathname);
  // The top bar's start: the page's nav group and item, layer patches applied (as the sidebar does).
  const active = activeItem(pathname, applyNav(NAV_ITEMS, use(EditionContext).edition.layers));
  const item = active
    ? {
        to: active.to,
        label: active.label,
        group: NAV_GROUPS.find((g) => g.id === active.group)?.label ?? null,
      }
    : null;
  // Overview and New chat have no bar on desktop (user, 2026-10-08): nothing to place, nowhere to step.
  const bare = BARE.test(pathname);
  // Controlled (v1c E1): the cookie wins; with none, the width default. Chat used to start as the icon rail so its
  // history column fit beside the nav; its history is the sidebar's drill-in panel now, so it needs the open sidebar.
  // Only a user toggle sets `choice`, and the sidebar writes the cookie then, so the default is never stored.
  const [choice, setChoice] = useState<boolean | undefined>(
    () => initialOpen ?? readSidebarCookie(),
  );
  const widthDefault = useSyncExternalStore(noop, widthDefaultOpen, serverWidthDefault);
  const open = choice ?? widthDefault;
  // The drill-in slot (sidebarPanel.ts): the sidebar hands out `target`, a page's SidebarPanel portals into it.
  const [target, setTarget] = useState<HTMLElement | null>(null);
  const [panels, setPanels] = useState(0);
  const inset = (
    <SidebarInset id="main" tabIndex={-1} className={cn("outline-none", fullHeight && "min-h-0")}>
      {/* One bar above every page (TopBar.tsx); on phones it also holds the menu button (§4.2). */}
      <ShellTopBar item={item} pathname={pathname} bare={bare}>
        <StaleBuildNotice />
        {fullHeight ? (
          <div className="min-h-0 flex-1">{children}</div>
        ) : (
          <div className="mx-auto w-full max-w-page px-4 py-4">{children}</div>
        )}
      </ShellTopBar>
    </SidebarInset>
  );
  return (
    <SidebarPanelContext value={{ target, setTarget, panels, setPanels }}>
      <SidebarProvider
        open={open}
        onOpenChange={setChoice}
        // A bare page has no bar to clear: its sticky rows and anchor jumps go to the top.
        className={cn(fullHeight ? "h-dvh" : "min-h-screen", bare && "md:[--topbar-h:0px]")}
      >
        {/* A button, not an `#main` link: a fragment navigation would add a history entry and make
          the router reload the route (review: red team). */}
        <Button
          variant="outline"
          size="sm"
          onClick={() => document.getElementById("main")?.focus()}
          className="sr-only z-50 focus:not-sr-only focus:fixed focus:top-2 focus:left-2"
        >
          {copy.skipToContent}
        </Button>
        <AppSidebar badges={badges} hidden={hidden} />
        {Split ? <Split path={pathname}>{inset}</Split> : inset}
        {end}
      </SidebarProvider>
    </SidebarPanelContext>
  );
}
