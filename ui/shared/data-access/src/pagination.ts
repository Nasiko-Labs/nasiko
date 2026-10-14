/**
 * Server pagination helpers (contract §9). A page is a window the server
 * returned; `total` is the server's known total, or "unknown" when the
 * backend does not report one. Virtualization never changes what is
 * downloaded; `retainPages` caps what is kept.
 */
export interface Page<T> {
  readonly rows: readonly T[];
  readonly offset: number;
  readonly limit: number;
  readonly total: number | "unknown";
}

export interface PageWindow<T> {
  readonly pages: ReadonlyMap<number, Page<T>>;
  readonly retainPages: number;
}

export function emptyWindow<T>(retainPages = 5): PageWindow<T> {
  return { pages: new Map(), retainPages };
}

/** Keeps at most `retainPages` pages, evicting the ones farthest from `focusOffset`. */
export function retain<T>(w: PageWindow<T>, page: Page<T>, focusOffset: number): PageWindow<T> {
  const pages = new Map(w.pages);
  pages.set(page.offset, page);
  if (pages.size > w.retainPages) {
    const victims = [...pages.keys()]
      .sort((a, b) => Math.abs(b - focusOffset) - Math.abs(a - focusOffset))
      .slice(0, pages.size - w.retainPages);
    for (const v of victims) pages.delete(v);
  }
  return { ...w, pages };
}

/** Changing a filter or sort invalidates every retained page (never mix windows). */
export function resetWindow<T>(w: PageWindow<T>): PageWindow<T> {
  return emptyWindow(w.retainPages);
}

export function describeTotal(total: number | "unknown", loaded: number): string {
  return total === "unknown" ? `${loaded} loaded (total unknown)` : `${loaded} of ${total}`;
}
