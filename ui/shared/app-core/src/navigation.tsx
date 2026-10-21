/** Browser navigation port. Product features do not depend on a rendering framework. */
import {
  createContext,
  useContext,
  useMemo,
  useRef,
  type ComponentPropsWithRef,
  type ReactNode,
} from "react";

export interface NavigateOptions {
  /** Scroll to the top after the move (default true). */
  scroll?: boolean;
  /** Pass a leave guard (the lab's `ignoreBlocker: true`): a move a dirty form itself makes, e.g. after a save. */
  ignoreBlocker?: boolean;
  /**
   * The new entry's history state (the lab's TanStack `navigate({ state })`): survives a reload and Back/Forward, not a
   * copied link. Without it the entry has none.
   */
  state?: HistoryState | undefined;
}

/** What a page may keep in a history entry. The router adds its own keys (`__TSR_index`); pages read their own. */
export interface HistoryState {
  /** The ordered ids of the list a detail view was opened from (siblings.ts, Previous/Next). */
  siblings?: readonly string[] | undefined;
  [key: string]: unknown;
}

export interface Navigation {
  push: (href: string, options?: NavigateOptions) => void;
  replace: (href: string, options?: NavigateOptions) => void;
  back: () => void;
  forward: () => void;
  refresh: () => void;
  prefetch: (href: string) => void;
}
export interface NavigationState {
  router: Navigation;
  pathname: string;
  search: string;
  /** The fragment without its `#` (the router's location; absent in a harness that has none). */
  hash?: string;
  /** The current entry's history state (absent in a harness that has none). */
  state?: HistoryState | undefined;
}
export const NavigationContext = createContext<NavigationState | null>(null);

/** A navigation held by a leave guard (the lab's `useBlocker({ withResolver: true })` result). */
export interface LeaveBlocker {
  /** A navigation is held (the lab's `blocker.status === "blocked"`). */
  blocked: boolean;
  /** Go where the held navigation was going. */
  proceed: () => void;
  /** Drop the held navigation and stay. */
  reset: () => void;
}
export interface LeaveBlockerOptions {
  when: boolean;
  /** Let search-only moves through. */
  samePath: boolean;
}
/**
 * The port's blocker: a hook the router implementation supplies (router-provider.tsx: TanStack's `useBlocker`; a test
 * harness its own over its history). Rules every implementation keeps: while `when`, hold any move to another URL
 * except `/login` (and, with `samePath`, except a search-only move), and ask before reload/close.
 */
export type UseLeaveBlocker = (options: LeaveBlockerOptions) => LeaveBlocker;
export const LeaveBlockerContext = createContext<UseLeaveBlocker | null>(null);
export const PathParamsContext = createContext<Record<string, string | string[]> | null>(null);

function useNavigation(): NavigationState {
  const value = useContext(NavigationContext);
  if (!value) throw new Error("NavigationProvider is missing");
  return value;
}
export function useRouter(): Navigation {
  return useNavigation().router;
}
export function usePathname(): string {
  return useNavigation().pathname;
}
/** The URL fragment without its `#`. */
export function useUrlHash(): string {
  return useNavigation().hash ?? "";
}
/** The current history entry's state (`HistoryState`); an empty object when it has none. */
export function useHistoryState(): HistoryState {
  return useNavigation().state ?? EMPTY_STATE;
}
const EMPTY_STATE: HistoryState = Object.freeze({});
export function useSearchParams(): URLSearchParams {
  const { search } = useNavigation();
  return useMemo(() => new URLSearchParams(search), [search]);
}
// The caller supplies the route-specific parameter shape, as matching belongs to the router.
export function useParams<
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters -- Route-specific caller type.
  T extends Record<string, string | string[]> = Record<string, string | string[]>,
>(): T {
  return (useContext(PathParamsContext) ?? {}) as T;
}

/** The lab's TanStack `defaultPreloadDelay`: a pointer passing over the sidebar does not preload every item. */
export const HOVER_DELAY_MS = 50;

export interface LinkProps extends ComponentPropsWithRef<"a"> {
  href: string;
  replace?: boolean;
  scroll?: boolean;
  /**
   * The entry's history state, or a function the click calls (the lab's TanStack `Link state`), so a list's rows can
   * share one stable function that reads the list as it is at click time (siblings.ts `useSiblingState`).
   */
  state?: HistoryState | (() => HistoryState) | undefined;
  children?: ReactNode;
}

/** The same-origin http(s) path of `href`, or null for anything that leaves the app. */
function appPath(href: string): string | null {
  const url = new URL(href, window.location.href);
  if (url.origin !== window.location.origin || !["http:", "https:"].includes(url.protocol))
    return null;
  return url.pathname + url.search + url.hash;
}

/**
 * Normal anchors retain downloads, modified clicks, external targets and the leave guard. In-app links preload their
 * route on intent, the lab's `defaultPreload: "intent"`: pointer hover after HOVER_DELAY_MS, keyboard focus and
 * touch/pen pointerdown at once (those have no hover). Nothing preloads on viewport entry.
 */
export function Link({
  href,
  replace,
  scroll,
  state,
  onClick,
  onPointerEnter,
  onPointerLeave,
  onPointerDown,
  onFocus,
  ...props
}: LinkProps) {
  const navigation = useContext(NavigationContext);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cancel = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
  };
  const preload = () => {
    if (!navigation || props.download !== undefined || (props.target && props.target !== "_self"))
      return;
    const path = appPath(href);
    if (path) navigation.router.prefetch(path);
  };
  return (
    <a
      {...props}
      href={href}
      onPointerEnter={(event) => {
        onPointerEnter?.(event);
        if (event.pointerType !== "mouse") return;
        cancel();
        timer.current = setTimeout(preload, HOVER_DELAY_MS);
      }}
      onPointerLeave={(event) => {
        onPointerLeave?.(event);
        cancel();
      }}
      onPointerDown={(event) => {
        onPointerDown?.(event);
        cancel();
        if (event.pointerType !== "mouse") preload();
      }}
      onFocus={(event) => {
        onFocus?.(event);
        preload();
      }}
      onClick={(event) => {
        onClick?.(event);
        if (
          !navigation ||
          event.defaultPrevented ||
          event.button !== 0 ||
          event.metaKey ||
          event.ctrlKey ||
          event.shiftKey ||
          event.altKey ||
          props.download !== undefined ||
          (props.target && props.target !== "_self")
        )
          return;
        const target = appPath(href);
        if (!target) return;
        event.preventDefault();
        const options: NavigateOptions = {};
        if (scroll !== undefined) options.scroll = scroll;
        if (state !== undefined) options.state = typeof state === "function" ? state() : state;
        navigation.router[replace ? "replace" : "push"](
          target,
          Object.keys(options).length ? options : undefined,
        );
      }}
    />
  );
}
export default Link;
