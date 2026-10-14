/**
 * Grouped filters (contract §13): draft values change freely; `applied`
 * changes only on an explicit Apply, so three control edits issue one query,
 * not three. Pure state machine; UI binds to it.
 */
export interface GroupedFilterState<T extends Record<string, unknown>> {
  readonly draft: T;
  readonly applied: T;
  /** Increments on every apply; queries key on this, not on draft. */
  readonly appliedVersion: number;
}

export function createGroupedFilters<T extends Record<string, unknown>>(
  initial: T,
): GroupedFilterState<T> {
  return { draft: initial, applied: initial, appliedVersion: 0 };
}

export function editDraft<T extends Record<string, unknown>>(
  s: GroupedFilterState<T>,
  patch: Partial<T>,
): GroupedFilterState<T> {
  return { ...s, draft: { ...s.draft, ...patch } };
}

export function isDirty<T extends Record<string, unknown>>(s: GroupedFilterState<T>): boolean {
  return JSON.stringify(s.draft) !== JSON.stringify(s.applied);
}

export function apply<T extends Record<string, unknown>>(
  s: GroupedFilterState<T>,
): GroupedFilterState<T> {
  if (!isDirty(s)) return s;
  return { draft: s.draft, applied: s.draft, appliedVersion: s.appliedVersion + 1 };
}

export function discardDraft<T extends Record<string, unknown>>(
  s: GroupedFilterState<T>,
): GroupedFilterState<T> {
  return { ...s, draft: s.applied };
}
