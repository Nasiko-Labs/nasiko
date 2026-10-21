// Ported from the legacy ui/ on development (3aac84f66) common/src/features/tour/TourHost.tsx (dd9fe0f64). Local
// edits: the pathname is the navigation port's `usePathname`; `meQuery` is `meQueryOptions(useProductScope())`; mounted
// by the workspace providers beside the build toasts (the lab's `_app.tsx`); Prettier formatting.
"use client";

/**
 * Mounted on every signed-in page (`_app.tsx`): opens this page's tour when asked (the top bar's tour button, or `?`),
 * and on its own for a first-time user (one the onboarding endpoint called first-time in this browser) once the setup
 * guide closes, finished or skipped: first the page's tour with the app tour (the sidebar) folded in, then each
 * page's tour on the first visit. Only once the page has loaded, so its targets are there. The Overview's tour always
 * carries the app tour. A tour belongs to its page: leaving the page closes it.
 */
import { useIsFetching, useQuery } from "@tanstack/react-query";
import { usePathname } from "@nasiko/app-core/navigation";
import { useEffect, useState } from "react";
import { useSlots } from "@nasiko/app-core/edition/edition-context";
import { useGuide, useGuideState } from "../onboarding/api";
import { meQueryOptions, useProductScope } from "@nasiko/data-access/product";
import { isTypingTarget } from "@nasiko/ui/lib";
import { askForTour, useTourAsks } from "@nasiko/app-core/tour";
import { TourOverlay } from "./index";
import { dueTour, markSeen, tourFor, type TourPrefs } from "./logic";
import { readPrefs, useTourPrefs, writePrefs } from "./store";

/** How long the page stays settled (no query in flight) before a tour opens on its own. */
const SETTLE_MS = 700;

/** Where `?` is someone else's key: typing, a menu or list's typeahead, or a dialog (the tour's own included). */
const OWN_KEYS =
  '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], [role="combobox"]';

interface Open {
  id: string;
  path: string;
  /** The app tour's steps come first. */
  withApp: boolean;
  /** A first-time user's first tour, opened on its own: it offers No thanks. */
  firstRun: boolean;
}

export function TourHost() {
  const pathname = usePathname();
  // The signed-in guard already loaded `me`; this only reads it.
  const sub = useQuery({
    ...meQueryOptions(useProductScope()),
    refetchOnMount: false,
    retryOnMount: false,
  }).data?.sub;
  const prefs = useTourPrefs(sub);
  const guide = useGuide();
  const guideOpen = useGuideState().open;
  const busy = useIsFetching() > 0;
  const layerTours = useSlots().tours;
  const [open, setOpen] = useState<Open | null>(null);

  // Asked for: this page's tour, or the app tour on a page without one.
  const asks = useTourAsks();
  const [answered, setAnswered] = useState(asks);
  if (asks !== answered) {
    setAnswered(asks);
    const id = tourFor(pathname, layerTours) ?? "app";
    setOpen({ id, path: pathname, withApp: id === "overview", firstRun: false });
  }
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "?" || e.metaKey || e.ctrlKey || e.altKey || e.repeat || e.defaultPrevented)
        return;
      if (isTypingTarget(e.target) || (e.target instanceof Element && e.target.closest(OWN_KEYS)))
        return;
      e.preventDefault();
      askForTour();
    };
    addEventListener("keydown", onKey);
    return () => {
      removeEventListener("keydown", onKey);
    };
  }, []);
  if (open && open.path !== pathname) setOpen(null);

  // A first-time user turns automatic tours on, once: after No thanks (which marks the app tour seen) they stay off.
  useEffect(() => {
    if (sub && guide.due && !prefs.auto && !prefs.seen.includes("app"))
      writePrefs(sub, { ...prefs, auto: true });
  }, [sub, guide.due, prefs]);

  const due =
    sub && !guide.due && !guideOpen && !open ? dueTour(prefs, pathname, layerTours) : null;
  const dueId = due?.id;
  const dueWithApp = due?.withApp ?? false;
  useEffect(() => {
    if (!dueId || busy) return;
    const t = setTimeout(() => {
      setOpen({
        id: dueId,
        path: pathname,
        withApp: dueWithApp || dueId === "overview",
        firstRun: dueWithApp,
      });
    }, SETTLE_MS);
    return () => {
      clearTimeout(t);
    };
  }, [dueId, dueWithApp, busy, pathname]);

  if (!open || !sub) return null;
  const end = (next: (p: TourPrefs) => TourPrefs) => {
    writePrefs(sub, next(readPrefs(sub)));
    setOpen(null);
  };
  return (
    <TourOverlay
      key={`${open.id}:${open.path}`}
      id={open.id}
      layer={layerTours.find((t) => t.id === open.id) ?? null}
      withApp={open.withApp}
      // The one place to turn automatic tours off.
      offerOptOut={open.firstRun}
      onEnd={() => {
        end((p) => markSeen(open.withApp ? markSeen(p, "app") : p, open.id));
      }}
      onOptOut={() => {
        end((p) => ({ ...markSeen(p, "app"), auto: false }));
      }}
    />
  );
}
