// Ported from nasiko-ui-lab@1b99bdda common/src/features/optimization/OptimizationHubPage.tsx. Local edits: `"use
// client"`; the router seam: `search`/`setSearch` come in as props from the route (TokenOps' pattern, for the lab's
// `useSearch`/`useSetSearch`), the hash is `useHash()` (@nasiko/app-core/url-state, over the port's `useUrlHash()`) and
// dropping `#settings` after the jump is `history.replaceState` to the same path and query (the lab's `navigate({
// hash: '', replace: true, resetScroll: false })`); the Your settings link is the navigation port's `Link` (`@nasiko/app-core/navigation`) to `useSettingsHref()`;
// `meQuery`/`strategyQuery`/`budgetQuery` take the scope and the return-tick refresh invalidates the scoped prefixes
// (`productKey(scope, 'tokenops', 'context-savings' | 'savings')`, `productKey(scope, 'optimization', 'history')`);
// Prettier formatting.
// The top bar (legacy ui/ on development, 3aac84f66: e8379db5a): the sticky bar sits under the shell's top bar
// (`top-(--topbar-h)`).
/**
 * The /optimization page (plans/feat-optimization-page.md; main sidebar, Observe after TokenOps, P7): what context
 * optimization saves, where it isn't working, and your settings. Built in steps: T1 hosts your settings here (P2, moved
 * from Settings → Account, whose old route redirects to #settings, R1D); T2 the sticky header (TokenOps' window, R7A;
 * the jump to your settings, R1D) and the lead (answer, sentence, trend chart); T3 Needs attention; T4 By agent and Biggest senders;
 * T5 your settings (sticky Save, preview on view, the compression sentence in the lead); T7 the Workspace footer
 * (superusers; EE adds Organization policy through the `optimizationWorkspace` slot).
 *
 * "Now" is frozen per window and moves when you come back to the tab, as on TokenOps (`useReturnTick`).
 */
"use client";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import Link from "@nasiko/app-core/navigation";
import { usePathname, useSearchParams } from "@nasiko/app-core/navigation";
import { useEffect, useMemo, useRef, useState } from "react";
import { Announcer } from "@nasiko/ui/composites";
import { PageHeader } from "@nasiko/ui/composites";
import { PanelError } from "@nasiko/ui/composites";
import { TimeControl } from "@nasiko/ui/composites";
import { useAgentsDirectory, useOwnedAgents } from "../agents/api";
import { bucketLabel } from "../tokenops/series";
import { resolveWindow } from "../tokenops/window";
import { meQueryOptions, productKey, useProductScope } from "@nasiko/data-access/product";
import { fmtShortDay } from "@nasiko/ui/lib";
import { useMediaQuery } from "@nasiko/ui/lib";
import { useFrozenNow, useReturnTick } from "@nasiko/app-core/useReturnTick";
import { useHash } from "@nasiko/app-core/url-state";
import {
  budgetQuery,
  strategyQuery,
  useSavingsByAgent,
  useSavingsTrend,
  useSettingsHistory,
  useTopRequests,
} from "./api";
import { BiggestSenders } from "./components/BiggestSenders";
import { ByAgent } from "./components/ByAgent";
import { Mechanisms } from "./components/Mechanisms";
import { useCompressionLine } from "./components/compressionLine";
import { Workspace } from "./components/Workspace";
import { useSettingsHref } from "./components/settingsHref";
import { Lead } from "./components/Lead";
import { NeedsAttention } from "./components/NeedsAttention";
import { copy } from "./copy";
import {
  leadState,
  parseSlice,
  sliceLabel,
  sliceOf,
  trendRows,
  weeklyPoints,
  type BucketSize,
} from "./lead";
import { OptimizationPage } from "./OptimizationPage";
import type { OptimizationSearch } from "./search";

export function OptimizationHubPage({
  search,
  setSearch,
}: {
  search: OptimizationSearch;
  setSearch: (patch: Partial<OptimizationSearch>, opts?: { replace?: boolean | undefined }) => void;
}) {
  const hash = useHash();
  const pathname = usePathname();
  const query = useSearchParams().toString();
  const scope = useProductScope();
  const settingsHeading = useRef<HTMLHeadingElement>(null);
  const settingsSection = useRef<HTMLElement>(null);
  // E4: the last-chat preview (a dry run that spends embedding calls) waits until Your settings is near the screen.
  // Without IntersectionObserver it runs at once.
  const [settingsSeen, setSettingsSeen] = useState(
    () => typeof IntersectionObserver === "undefined",
  );
  // A jump made before the blocks above had their real height: scrolled again once they settle (review: red team).
  const jumpPending = useRef(false);
  // Arriving at #settings (the header link, the old Settings route) puts focus on the section it names (eng E5(2)).
  // The hash has then done its job and is dropped (replace), so Back/Forward into this entry or a later search move
  // never jumps again, and the next click on the header link is a fresh arrival (review: red team, design).
  useEffect(() => {
    if (hash !== "settings") return;
    const h = settingsHeading.current;
    h?.scrollIntoView({ block: "start" });
    h?.focus({ preventScroll: true });
    jumpPending.current = true;
    // The lab's `navigate({ hash: '', search: (prev) => prev, replace: true, resetScroll: false })`: the same URL
    // without its fragment, through the History API (no scroll, no server round trip).
    window.history.replaceState(null, "", query ? `${pathname}?${query}` : pathname);
  }, [hash, pathname, query]);

  const returnTick = useReturnTick();
  const queryClient = useQueryClient();
  // As on TokenOps: a preset window moves with the frozen "now"; a custom or last-month window keeps its key, so coming
  // back re-reads the page's savings and history explicitly (review: adversarial).
  useEffect(() => {
    if (returnTick === 0) return;
    void queryClient.invalidateQueries({
      queryKey: productKey(scope, "tokenops", "context-savings"),
    });
    void queryClient.invalidateQueries({ queryKey: productKey(scope, "tokenops", "savings") });
    void queryClient.invalidateQueries({ queryKey: productKey(scope, "optimization", "history") });
  }, [returnTick, queryClient, scope]);
  const now = useFrozenNow(search.preset, search.from, search.to, returnTick);
  const win = useMemo(
    () => resolveWindow({ preset: search.preset, from: search.from, to: search.to }, now),
    [search.preset, search.from, search.to, now],
  );
  const me = useQuery(meQueryOptions(scope));
  const ready = me.isSuccess;
  const superuser = me.data?.is_superuser === true;
  const directory = useAgentsDirectory(ready);
  const trend = useSavingsTrend(win, ready);
  const history = useSettingsHistory(win, ready && trend.data?.kind === "ready");
  const state = leadState({
    result: trend.data,
    failed: trend.isError,
    agentCount: directory.data ? directory.data.length : null,
  });
  const savings = trend.data?.kind === "ready" ? trend.data.data : null;
  const bucket = savings?.series?.bucket ?? (win.params.range === "24h" ? "hour" : "day");
  const tableRows = uniqueLabels(
    (savings?.series ? trendRows(savings.series.points, savings.recorded_since) : []).map((r) => ({
      ...r,
      label: bucketLabel(new Date(r.iso), bucket),
    })),
  );
  // R6B: below 640 px the chart draws weeks (the Table view keeps the server's buckets); 24h stays hourly.
  const phone = !useMediaQuery("(min-width: 40rem)");
  const weekly = phone && bucket === "day" && !!savings?.series;
  const unit: BucketSize = weekly ? "week" : bucket;
  const rows = weekly
    ? trendRows(weeklyPoints(savings.series?.points ?? []), savings.recorded_since).map((r) => ({
        ...r,
        // The axis has room for a date only ("Week of …" was cut to "Week ..." on a phone, QA ISSUE-003).
        label: fmtShortDay(r.iso),
        title: copy.lead.chart.week(fmtShortDay(r.iso)),
      }))
    : tableRows;
  const slice = parseSlice(search.slice, win);
  const compression = useCompressionLine();
  const noAgents = directory.data?.length === 0;
  const byAgent = useSavingsByAgent(win, ready);
  // CX-V3c takes from/to inside the window: a week counted back from today, or the first hour of 24h, can start
  // before it, so the interval sent is the slice clipped to the window (review: api-contract + core).
  const clipped = slice
    ? {
        ...slice,
        from: slice.from < win.start.toISOString() ? win.start.toISOString() : slice.from,
        to: slice.to > win.end.toISOString() ? win.end.toISOString() : slice.to,
      }
    : null;
  const top = useTopRequests(win, clipped, ready);
  // Watched only once the blocks above have their real height: on first paint they are short skeletons, and the
  // section would look "near the screen" before anyone scrolled.
  // The agent lists decide the Needs attention strip's height, so they count too (review: performance).
  const owned = useOwnedAgents(me.data?.sub);
  const blocksSettled =
    me.isError ||
    (!trend.isPending &&
      !byAgent.isPending &&
      !top.isPending &&
      !directory.isPending &&
      !(me.data?.sub && owned.isPending));
  useEffect(() => {
    if (!blocksSettled || !jumpPending.current) return;
    jumpPending.current = false;
    const h = settingsHeading.current;
    if (h && document.activeElement === h) h.scrollIntoView({ block: "start" });
  }, [blocksSettled]);
  useEffect(() => {
    const el = settingsSection.current;
    if (settingsSeen || !blocksSettled || !el) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) setSettingsSeen(true);
      },
      { rootMargin: "200px" },
    );
    io.observe(el);
    return () => {
      io.disconnect();
    };
  }, [settingsSeen, blocksSettled]);
  const period = win.label;
  const today = now.toISOString().slice(0, 10);

  return (
    // R6x: the page's one polite live region (shared with the LLM router).
    <Announcer>
      <div className="mx-auto flex w-full max-w-5xl flex-col gap-6">
        {/* TokenOps' sticky bar: title, the window, and the jump to your settings (R1D, R7A). */}
        <div className="sticky top-(--topbar-h) z-20 -mx-4 border-b border-border bg-background/95 px-4 py-3 backdrop-blur">
          <PageHeader
            title={copy.title}
            description={phone ? undefined : <SettingsLink />}
            actions={
              <TimeControl
                preset={search.preset}
                invalid={search.preset === "custom" && win.preset !== "custom"}
                from={search.from}
                to={search.to}
                today={today}
                // C5: a new window clears the picked bar.
                onChange={(n) => {
                  setSearch(
                    { preset: n.preset, from: n.from, to: n.to, slice: undefined },
                    { replace: search.preset === "custom" && n.preset === "custom" },
                  );
                }}
              />
            }
          />
        </div>
        {/* Without your account the blocks can't scope anything: one error with Retry, not skeletons that never end (R2C).
          A 401 is the app's expiry path, so it never shows here. */}
        {me.isError ? (
          <PanelError error={me.error} onRetry={() => void me.refetch()} what={copy.hub.account} />
        ) : (
          <>
            <Lead
              state={state}
              error={trend.error}
              onRetry={() => void trend.refetch()}
              win={{ ...win, from: search.from, to: search.to }}
              superuser={superuser}
              rows={rows}
              tableRows={tableRows}
              unit={unit}
              tableUnit={bucket}
              // R6B: on a phone the header is the title and the window; the jump to your settings sits here.
              under={
                phone ? (
                  <p className="m-0 text-sm text-muted-foreground">
                    <SettingsLink />
                  </p>
                ) : null
              }
              history={history.data}
              compression={compression}
              // R7C, C5: a bar picks its interval, pushed so Back undoes it.
              onSelect={(iso, size) => {
                setSearch({ slice: sliceOf(iso, size) });
              }}
            />
            <NeedsAttention />
            <ByAgent
              result={byAgent.data}
              pending={byAgent.isPending}
              error={byAgent.error}
              onRetry={() => void byAgent.refetch()}
              period={period}
              sort={search.sort}
              all={search.all === true}
              // R7B: sort and the expander replace history (TokenOps' table rule).
              onSort={(sort) => {
                setSearch({ sort }, { replace: true });
              }}
              onShowAll={(all) => {
                setSearch({ all: all || undefined }, { replace: true });
              }}
              noAgents={noAgents}
            />
            {/* §11: what cuts tokens, where each is set and its state (B12). */}
            <Mechanisms win={win} period={period} superuser={superuser} enabled={ready} />
            <BiggestSenders
              result={top.data}
              pending={top.isPending}
              error={top.error}
              onRetry={() => void top.refetch()}
              period={period}
              // The chip names what was asked for: the bar clipped to the window (review: adversarial).
              slice={clipped ? sliceLabel(clipped) : null}
              onClear={() => {
                setSearch({ slice: undefined }, { replace: true });
              }}
              nameOf={(id, raw) => {
                const a = directory.byId.get(id);
                return a ? a.display_name || a.name : raw;
              }}
              noAgents={noAgents}
            />
          </>
        )}
        <section
          ref={settingsSection}
          id="settings"
          aria-labelledby="optimization-settings-h"
          className="scroll-mt-24"
        >
          <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1 pb-3">
            <h2
              id="optimization-settings-h"
              ref={settingsHeading}
              tabIndex={-1}
              // The jump scrolls the heading itself, so the margin clears the sticky header here (review: design).
              className="scroll-mt-24 text-sm font-semibold outline-none"
            >
              {copy.hub.yourSettings}
            </h2>
            <span className="text-xs text-muted-foreground">{copy.hub.yourSettingsNote}</span>
          </div>
          <OptimizationPage previewEnabled={settingsSeen || hash === "settings"} />
        </section>
        <Workspace />
      </div>
    </Announcer>
  );
}

/** R1D: "Your settings: PACMS · Medium", a jump to the form; nothing until both preferences have loaded. */
function SettingsLink() {
  const scope = useProductScope();
  const strategy = useQuery(strategyQuery(scope));
  const budget = useQuery(budgetQuery(scope));
  const href = useSettingsHref();
  if (!strategy.data || !budget.data) return null;
  const summary =
    strategy.data.enabled === false
      ? copy.hub.off
      : `${copy.strategies[strategy.data.strategy].name} · ${copy.level(budget.data.level)}`;
  return (
    <Link
      href={href}
      aria-label={copy.hub.settingsLinkLabel(summary)}
      // A standalone link under the lead on a phone: a full-size target there (review: design).
      className="hover:underline hover:underline-offset-4 pointer-coarse:inline-flex pointer-coarse:min-h-11 pointer-coarse:items-center"
    >
      {copy.hub.settingsLink} <span className="font-medium text-foreground">{summary}</span>
    </Link>
  );
}

/**
 * The chart's categories are its labels (the kit's band scale), so they must be unique: a rolling 24 h window holds 25
 * hourly buckets whose first and last read the same hour. A repeated label gets its day (review: Codex).
 */
function uniqueLabels<T extends { iso: string; label: string }>(rows: T[]): T[] {
  const seen = new Map<string, number>();
  for (const r of rows) seen.set(r.label, (seen.get(r.label) ?? 0) + 1);
  return rows.map((r) =>
    (seen.get(r.label) ?? 0) > 1 ? { ...r, label: `${fmtShortDay(r.iso)}, ${r.label}` } : r,
  );
}
