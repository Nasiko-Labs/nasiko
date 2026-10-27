/**
 * The Flows list's last search (plans/feat-flows.md F20): "← Flows" on a flow page goes back to the list as the user
 * left it (search, kind, status, window). Client state that outlives the route, so a zustand store (CLAUDE.md).
 */
import { create } from 'zustand'
import type { FlowsSearch } from './search'

export const useListSearch = create<{ search: FlowsSearch | null }>(() => ({ search: null }))

export const rememberListSearch = (search: FlowsSearch) => useListSearch.setState({ search })
