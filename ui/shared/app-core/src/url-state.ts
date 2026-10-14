// URL search state over the navigation port (no lab counterpart: the lab's pages read `Route.useSearch()` and wrote
// through `useSetSearch` in common/src/lib/search.ts, with TanStack Router's `__TSR_index` for the history position).
// Local edits: n/a (new file).

/**
 * `useSearchState(schema)` gives a page its validated search (every key parsed through the page's zod schema, junk
 * falling back) and `navigate({ search, replace })` with the lab's semantics: a push unless `replace`, never a scroll.
 * `setSearch(patch, { replace })` is the lab's `useSetSearch` shape. Keys whose value is the schema's default (or
 * undefined) are left out of the URL, so a drill-down's URL stays as short as a hand-typed one.
 *
 * History position: the lab asked the router for `location.state.__TSR_index` to decide whether closing a panel it
 * opened may go Back (the current entry is still the one the open pushed) or must replace (a deep link, or a later
 * push). The SPA runs on the same router, which stamps that index into every entry it writes, so `historyIndex()` reads
 * it back from `history.state`. The router is the only writer of the history; nothing here wraps the History API.
 */
import { usePathname, useRouter, useSearchParams, useUrlHash } from "@nasiko/app-core/navigation";
import { useCallback, useMemo } from "react";

/** The key the router stamps into `history.state` (TanStack Router's `__TSR_index`). Test harnesses stamp it too. */
export const HISTORY_INDEX_KEY = "__TSR_index";

/** The current entry's position in the session history, as the router numbered it (0 when it has not). */
export function historyIndex(): number {
  if (typeof window === "undefined") return 0;
  const v = (window.history.state as { [HISTORY_INDEX_KEY]?: unknown } | null)?.[HISTORY_INDEX_KEY];
  return typeof v === "number" && Number.isFinite(v) ? v : 0;
}

/** The URL fragment without its `#` (the lab's `useLocation({ select: (l) => l.hash })`), from the navigation port. */
export function useHash(): string {
  return useUrlHash();
}

export interface SearchSchema<S> {
  parse(input: unknown): S;
}

/** Stringify one search value the way the lab's router did (strings as-is, booleans and numbers as JSON). */
function stringify(v: unknown): string | null {
  if (v === undefined || v === null) return null;
  if (typeof v === "string") return v;
  if (typeof v === "boolean" || typeof v === "number") return String(v);
  return JSON.stringify(v);
}

/**
 * The query string for `search` (no leading `?`), omitting undefined values and, when `defaults` is given, keys that
 * equal their default. Values are encoded by URLSearchParams.
 */
export function searchString(
  search: Record<string, unknown>,
  defaults?: Record<string, unknown>,
): string {
  const qs = new URLSearchParams();
  for (const [k, v] of Object.entries(search)) {
    const s = stringify(v);
    if (s === null) continue;
    if (defaults && k in defaults && Object.is(defaults[k], v)) continue;
    qs.set(k, s);
  }
  return qs.toString();
}

/** `pathname?search` for a link, as `searchString` builds it. */
export function hrefFor(pathname: string, search: Record<string, unknown>): string {
  const qs = searchString(search);
  return qs ? `${pathname}?${qs}` : pathname;
}

export interface NavigateOptions<S> {
  /** The next search: a whole object, or an updater over the current validated search. */
  search: Partial<S> | ((prev: S) => Partial<S>);
  /** Replace the current entry instead of pushing one. */
  replace?: boolean | undefined;
}

export interface SearchState<S extends object> {
  search: S;
  navigate: (opts: NavigateOptions<S>) => void;
  /** The lab's `setSearch`: merge `patch` into the current search; push unless `replace`. */
  setSearch: (patch: Partial<S>, opts?: { replace?: boolean | undefined }) => void;
  /** The lab's `router.history.back()`. */
  back: () => void;
  /** The lab's `router.state.location.state.__TSR_index`. */
  historyIndex: () => number;
}

export function useSearchState<S extends object>(schema: SearchSchema<S>): SearchState<S> {
  const router = useRouter();
  const pathname = usePathname();
  const params = useSearchParams();
  const raw = params.toString();
  const search = useMemo(
    () => schema.parse(Object.fromEntries(new URLSearchParams(raw).entries())),
    [schema, raw],
  );
  const defaults = useMemo(() => schema.parse({}) as Record<string, unknown>, [schema]);
  const navigate = useCallback(
    (opts: NavigateOptions<S>) => {
      const next = typeof opts.search === "function" ? opts.search(search) : opts.search;
      const qs = searchString(next, defaults);
      const href = qs ? `${pathname}?${qs}` : pathname;
      // Through the router, so a leave guard can hold it and the router numbers the entry.
      if (opts.replace) router.replace(href, { scroll: false });
      else router.push(href, { scroll: false });
    },
    [pathname, search, defaults, router],
  );
  const setSearch = useCallback(
    (patch: Partial<S>, opts?: { replace?: boolean | undefined }) => {
      navigate({
        search: (prev) => ({ ...prev, ...patch }),
        replace: opts?.replace ?? false,
      });
    },
    [navigate],
  );
  const back = useCallback(() => {
    router.back();
  }, [router]);
  return { search, navigate, setSearch, back, historyIndex };
}
