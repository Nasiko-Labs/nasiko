// Ported from nasiko-ui-lab@1b99bdda common/src/features/agents/api.ts. Local edits: every query key is scoped,
// `productKey(scope, "agents", …the lab's parts)` (`agentKeys.*` and the query-option builders `directoryQuery`,
// `catalogQuery`, `ownedQuery`, `agentDetailQuery` take the scope first; hooks read it from `useProductScope()`), so
// the lab's `AGENTS_DIRECTORY_KEY` constant is `agentKeys.directory(scope)` and the pilot's `AGENTS_FEATURE` names
// the feature; prefix invalidations use `agentKeys.all(scope)`/`productKey` prefixes and the delete's `queryKey[2]
// === id` predicate reads index 6 (the scope adds four parts); the TokenOps savings invalidations are
// `productKey(scope, "tokenops", …)`; `ApiError` is `ProductApiError`; `qc.query` for the lab's deprecated `fetchQuery` (same fetch-if-stale); named
// eslint-disables on the defensive reads of server data; Prettier formatting.
/**
 * Agent queries and mutations (plan §5, §6). Facts that shape them:
 * - The list is a bare array paged by offset over `created_at DESC`, so pages are deduped by
 *   id (an insert between page fetches shifts offsets).
 * - Detail status isn't reconciled with the runtime; the list is. `useAgentStatus` reads the
 *   one list page that holds the agent (`owner=<owner_id>`) instead of polling every page.
 * - Lifecycle routes take the UUID (their name lookup isn't owner-scoped).
 */
import {
  infiniteQueryOptions,
  queryOptions,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { useEffect, useMemo } from "react";
import type { z } from "zod";
import { create } from "zustand";
import type { Scope } from "@nasiko/data-access";
import {
  apiData,
  apiFetch,
  isUnauthorized,
  onCacheReset,
  ProductApiError as ApiError,
  productKey,
  productPrefix,
  useProductScope,
  withQuery,
} from "@nasiko/data-access/product";
import { isSignedOutLocally, isSigningOut, signInGeneration } from "@nasiko/app-core/session";
import { createLimiter } from "../observability/limiter";
import type { LogLine, SessionListResponse, SessionSummary } from "../observability/types";
import type { components } from "@nasiko/data-access/product/schema";
import {
  grantsBodySchema,
  grantWrite,
  normalizeGrants,
  type AccessGrants,
  type GrantsBody,
} from "./grants";
import { normalizeDetail, type AgentView } from "./normalize";
import { displayStatus, isHarness, type DisplayStatus } from "./status";
import {
  AGENTS_MAX_PAGES,
  AGENTS_PAGE,
  DEPLOYMENT_STALE_MS,
  DETAIL_STALE_MS,
  DIRECTORY_STALE_MS,
  ERROR_LOG_LIMIT,
  ERROR_LOG_WINDOW_MS,
  ERROR_LOGS_STALE_MS,
  LIVE_CARD_STALE_MS,
  LOG_LIMIT,
  POLL_MS,
  SESSIONS_FALLBACK_MS,
  SESSIONS_SCAN,
  USER_SEARCH_MAX,
  USER_SEARCH_MIN,
  USERS_LIMIT,
  WATCH_CAP_MS,
} from "./tuning";
import { agentDetailSchema, agentStatsSchema, usage24hSchema } from "./types";
import type {
  Agent,
  AgentDetailResponse,
  AgentGrant,
  AgentResources,
  AgentStats,
  AgentVersion,
  DeletedAgent,
  DeploymentRow,
  RollbackResponse,
  SecretName,
  Unavailable,
  UserGrant,
  UserRow,
  UserSearchResult,
} from "./types";
import {
  expireWatch,
  isWatching,
  startWatch,
  stepWatch,
  type Watch,
  type WatchKind,
} from "./watch";

const OBS = "/api/observability";

/** The agent directory's feature (`productKey(scope, AGENTS_FEATURE)`), for pages outside this module that refresh it. */
export const AGENTS_FEATURE = "agents";

const k = (scope: Scope, ...parts: readonly unknown[]) =>
  productKey(scope, AGENTS_FEATURE, ...parts);

export const agentKeys = {
  /** Every agents key of the scope (prefix). */
  all: (scope: Scope) => productPrefix(scope, AGENTS_FEATURE),
  // Shared with Sessions' directory (same key, same data).
  directory: (scope: Scope) => k(scope),
  catalog: (scope: Scope) => k(scope, "catalog"),
  owned: (scope: Scope, owner: string) => k(scope, "owned", owner),
  statusPage: (scope: Scope, owner: string) => k(scope, "status-page", owner),
  detail: (scope: Scope, id: string) => k(scope, "detail", id),
  deployment: (scope: Scope, id: string) => k(scope, "deployment", id),
  versions: (scope: Scope, id: string) => k(scope, "versions", id),
  grants: (scope: Scope, id: string) => k(scope, "grants", id),
  userGrants: (scope: Scope, id: string) => k(scope, "grants-users", id),
  agentGrants: (scope: Scope, id: string) => k(scope, "grants-agents", id),
  secrets: (scope: Scope, id: string) => k(scope, "secrets", id),
  stats: (scope: Scope, id: string) => k(scope, "stats", id),
  resources: (scope: Scope, id: string) => k(scope, "resources", id),
  errorLogs: (scope: Scope, id: string) => k(scope, "error-logs", id),
  logs: (scope: Scope, id: string, level: string) => k(scope, "logs", id, level),
  liveCard: (scope: Scope, id: string) => k(scope, "live-card", id),
  sessions: (scope: Scope, name: string, createdAt: string) =>
    k(scope, "sessions", name, createdAt),
  usage24h: (scope: Scope) => k(scope, "usage-24h"),
  users: (scope: Scope) => k(scope, "users"),
  userSearch: (scope: Scope, q: string) => k(scope, "user-search", q),
  settings: (scope: Scope) => k(scope, "settings"),
};

/** Index of the lab's key part `i` in a scoped key (`["nasiko", "product", workspace, epoch, ...lab parts]`). */
const PART = (i: number) => i + 4;

// ─── lists ──────────────────────────────────────────────────────────────────

const listPath = (offset: number, owner?: string) =>
  withQuery("/api/agents", { limit: AGENTS_PAGE, offset, owner });

async function fetchPage(offset: number, signal: AbortSignal, owner?: string): Promise<Agent[]> {
  const page = await apiFetch<unknown>(listPath(offset, owner), { signal });
  if (!Array.isArray(page))
    throw new ApiError(
      200,
      page,
      listPath(offset, owner),
      "GET /api/agents → unexpected response (not an array)",
    );
  return page as Agent[];
}

/** Drop rows already seen (by id), keeping first-seen order. */
export function dedupeById<T extends { id: string }>(rows: T[]): T[] {
  const seen = new Set<string>();
  return rows.filter((r) => (seen.has(r.id) ? false : (seen.add(r.id), true)));
}

/** Every page of GET /api/agents (limit clamps to 100), optionally one owner's. */
async function fetchAllAgents(signal: AbortSignal, owner?: string): Promise<Agent[]> {
  const all: Agent[] = [];
  for (let offset = 0; offset < AGENTS_PAGE * AGENTS_MAX_PAGES; offset += AGENTS_PAGE) {
    const page = await fetchPage(offset, signal, owner);
    all.push(...page);
    if (page.length < AGENTS_PAGE) break;
  }
  return dedupeById(all);
}

interface DirectoryIndex {
  byName: Map<string, Agent>;
  byId: Map<string, Agent>;
  byNameAll: Map<string, Agent[]>;
}
const EMPTY_INDEX: DirectoryIndex = { byName: new Map(), byId: new Map(), byNameAll: new Map() };
/** Built once per fetched list, shared by every caller (a table can hold hundreds of AgentLinks). */
const indexCache = new WeakMap<Agent[], DirectoryIndex>();
function indexFor(rows: Agent[] | undefined): DirectoryIndex {
  if (!rows) return EMPTY_INDEX;
  let idx = indexCache.get(rows);
  if (!idx) {
    idx = { byName: new Map(), byId: new Map(), byNameAll: new Map() };
    for (const a of rows) {
      idx.byName.set(a.name, a);
      idx.byId.set(a.id, a);
      const same = idx.byNameAll.get(a.name);
      if (same) same.push(a);
      else idx.byNameAll.set(a.name, [a]);
    }
    indexCache.set(rows, idx);
  }
  return idx;
}

/** Every agent the caller can see, by id, raw name and all agents sharing a name. */
export function directoryQuery(scope: Scope) {
  return queryOptions({
    queryKey: agentKeys.directory(scope),
    queryFn: ({ signal }) => fetchAllAgents(signal),
    staleTime: DIRECTORY_STALE_MS,
    meta: { path: "/api/agents" },
  });
}

export function useAgentsDirectory(enabled = true) {
  const scope = useProductScope();
  const q = useQuery({ ...directoryQuery(scope), enabled });
  return { ...q, ...indexFor(q.data) };
}

export function catalogQuery(scope: Scope) {
  return infiniteQueryOptions({
    queryKey: agentKeys.catalog(scope),
    queryFn: ({ pageParam, signal }) => fetchPage(pageParam, signal),
    initialPageParam: 0,
    getNextPageParam: (last, pages) =>
      last.length === AGENTS_PAGE && pages.length < AGENTS_MAX_PAGES
        ? pages.length * AGENTS_PAGE
        : undefined,
    staleTime: DIRECTORY_STALE_MS,
    meta: { path: "/api/agents" },
  });
}

/**
 * The catalog: page 1 renders at once, later pages follow ("Loading more…"); a failed later
 * page keeps what loaded. The auto-advance effect depends on pages.length (learning:
 * batched renders can skip the intermediate fetching state).
 */
export function useCatalogAgents() {
  const q = useInfiniteQuery(catalogQuery(useProductScope()));
  const pages = q.data?.pages.length ?? 0;
  const { hasNextPage, isFetchingNextPage, isFetchNextPageError, fetchNextPage } = q;
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage && !isFetchNextPageError) void fetchNextPage();
  }, [pages, hasNextPage, isFetchingNextPage, isFetchNextPageError, fetchNextPage]);
  const agents = useMemo(() => dedupeById((q.data?.pages ?? []).flat()), [q.data]);
  return { ...q, agents };
}

/** A row Deploying for less than the watch cap; one stuck longer (a lost build job) stops the polling. */
export const hasFreshDeploying = (rows: Agent[] | undefined, now: number) =>
  !!rows?.some(
    (a) =>
      displayStatus(a.status, isHarness(a)) === "deploying" &&
      now - Date.parse(a.updated_at) < WATCH_CAP_MS,
  );

/** Your agents: one owner's rows, polled every 5 s while one is Deploying or a row is being watched. */
export const ownedQuery = (scope: Scope, owner: string) =>
  queryOptions({
    queryKey: agentKeys.owned(scope, owner),
    queryFn: ({ signal }) => fetchAllAgents(signal, owner),
    meta: { path: "/api/agents?owner=" },
  });

export function useOwnedAgents(owner: string | undefined, watching = false) {
  return useQuery({
    ...ownedQuery(useProductScope(), owner ?? ""),
    enabled: !!owner,
    // Pure: reads only the query's own data and the clock (learning: refetchInterval runs on every render).
    refetchInterval: (query) =>
      watching || hasFreshDeploying(query.state.data, Date.now()) ? POLL_MS : false,
  });
}

// ─── one agent ──────────────────────────────────────────────────────────────

export function agentDetailQuery(scope: Scope, id: string) {
  return queryOptions({
    queryKey: agentKeys.detail(scope, id),
    queryFn: ({ signal }) =>
      apiData<AgentDetailResponse>(`/api/agents/${id}`, { signal, schema: agentDetailSchema }),
    staleTime: DETAIL_STALE_MS,
    meta: { path: `/api/agents/${id}` },
  });
}

/** `poll`: while a watch runs and the agent isn't on its owner's first list page, the detail is the only status source. */
export function useAgentDetail(id: string, enabled = true, poll = false) {
  return useQuery({
    ...agentDetailQuery(useProductScope(), id),
    enabled,
    select: normalizeDetail,
    refetchInterval: poll ? POLL_MS : false,
  });
}

/**
 * The detail page's display status: the owner-scoped list page (reconciled server-side)
 * when the agent is on it, else the detail's own `status`. Polls only while watching.
 */
export function useAgentStatus(
  agent: AgentView | undefined,
  watching: boolean,
): {
  display: DisplayStatus | undefined;
  raw: string | undefined;
  row: Agent | undefined;
  failed: boolean;
  updatedAt: number;
  errorAt: number;
} {
  const owner = agent?.ownerId ?? "";
  const q = useQuery({
    queryKey: agentKeys.statusPage(useProductScope(), owner),
    queryFn: ({ signal }) => fetchPage(0, signal, owner),
    enabled: !!agent && !agent.isHarness,
    staleTime: DETAIL_STALE_MS,
    refetchInterval: watching ? POLL_MS : false,
    meta: { path: "/api/agents?owner=" },
  });
  if (!agent)
    return {
      display: undefined,
      raw: undefined,
      row: undefined,
      failed: false,
      updatedAt: 0,
      errorAt: 0,
    };
  const row = q.data?.find((a) => a.id === agent.id);
  const raw = row?.status ?? agent.status;
  return {
    display: displayStatus(raw, agent.isHarness),
    raw,
    row,
    failed: q.isError,
    updatedAt: q.dataUpdatedAt,
    errorAt: q.errorUpdatedAt,
  };
}

/** `/deployment`: a row, `{available:false}`, or `null` when there's no live row (404). Shared with the Overview's fleet health. */
function deploymentQuery(scope: Scope, id: string) {
  return queryOptions({
    queryKey: agentKeys.deployment(scope, id),
    queryFn: async ({ signal }): Promise<DeploymentRow | Unavailable | null> => {
      try {
        const body = await apiFetch<DeploymentRow | Unavailable>(`/api/agents/${id}/deployment`, {
          signal,
        });
        // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- an empty body is null at run time
        return body ?? null;
      } catch (err) {
        if (err instanceof ApiError && err.status === 404) return null;
        throw err;
      }
    },
    staleTime: DEPLOYMENT_STALE_MS,
  });
}

export function useDeployment(id: string, enabled: boolean, watching = false) {
  return useQuery({
    ...deploymentQuery(useProductScope(), id),
    enabled,
    refetchInterval: watching ? POLL_MS : false,
  });
}

export const isUnavailable = (v: unknown): v is Unavailable =>
  !!v && typeof v === "object" && (v as { available?: unknown }).available === false;

export function useVersions(id: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.versions(useProductScope(), id),
    queryFn: ({ signal }) => apiData<AgentVersion[]>(`/api/agents/${id}/versions`, { signal }),
    enabled,
    select: (vs) => [...vs].sort((a, b) => b.created_at.localeCompare(a.created_at)),
  });
}

/** Parse guard: the grants routes are OSS-mounted; EE serves its own shapes at these paths. */
function arrayOf<T>(check: (x: unknown) => boolean, path: string) {
  return (body: unknown): T[] => {
    if (!Array.isArray(body) || !body.every(check))
      throw new ApiError(200, body, path, `${path} → unexpected response shape`);
    return body as T[];
  };
}
const isObj = (x: unknown): x is Record<string, unknown> => !!x && typeof x === "object";

/** Both editions' `/grants` (grants.ts): the summary, tagged with the edition that answered. */
const grantsQuery = (scope: Scope, id: string) =>
  queryOptions({
    queryKey: agentKeys.grants(scope, id),
    queryFn: async ({ signal }): Promise<AccessGrants> => {
      const path = `/api/agents/${id}/grants`;
      const body = await apiFetch<GrantsBody>(path, { signal, schema: grantsBodySchema });
      const summary = normalizeGrants(body, id);
      if (summary.edition === "ee") {
        // EE can be public with no 'public' grant row (grants.rs revoke_public); agents.is_public is the truth.
        const vis = await apiFetch<unknown>(`/api/agents/${id}/visibility`, { signal });
        if (
          vis &&
          typeof vis === "object" &&
          typeof (vis as { is_public?: unknown }).is_public === "boolean"
        )
          return { ...summary, is_public: (vis as { is_public: boolean }).is_public };
      }
      return summary;
    },
  });

export function useGrants(id: string, enabled: boolean) {
  return useQuery({ ...grantsQuery(useProductScope(), id), enabled });
}

export function useUserGrants(id: string, enabled: boolean) {
  const qc = useQueryClient();
  const scope = useProductScope();
  const path = `/api/agents/${id}/grants/users`;
  // `username` is null for a grant to a deleted user (LEFT JOIN, agents/grants.rs:226-245): still removable.
  const parse = arrayOf<UserGrant>(
    (x) =>
      isObj(x) &&
      typeof x.user_id === "string" &&
      (typeof x.username === "string" || x.username == null),
    path,
  );
  return useQuery({
    queryKey: agentKeys.userGrants(scope, id),
    queryFn: async ({ signal }) => {
      // Once the edition is known to be OSS, fetch at once instead of waiting on /grants (refetches after a write).
      if (qc.getQueryData<AccessGrants>(agentKeys.grants(scope, id))?.edition === "oss")
        return parse(await apiFetch<unknown>(path, { signal }));
      // query (the lab's fetchQuery), not ensureQueryData: after a write invalidates /grants, the derived lists must see the new rows.
      const grants = await qc.query({ ...grantsQuery(scope, id), staleTime: 5_000 });
      if (grants.edition === "oss") return parse(await apiFetch<unknown>(path, { signal }));
      // EE has no /grants/users read: the grant rows carry ids, and /api/agents/{id}/users names everyone with access.
      if (!grants.user_grants.length) return [];
      const users = await apiFetch<unknown>(`/api/agents/${id}/users`, { signal });
      const names = new Map(
        (Array.isArray(users) ? users : [])
          .filter(isObj)
          .map((u) => [String(u.id), typeof u.username === "string" ? u.username : null]),
      );
      return grants.user_grants.map((uid) => ({ user_id: uid, username: names.get(uid) ?? null }));
    },
    enabled,
  });
}

export function useAgentGrants(id: string, enabled: boolean) {
  const qc = useQueryClient();
  const scope = useProductScope();
  const path = `/api/agents/${id}/grants/agents`;
  const parse = arrayOf<AgentGrant>((x) => isObj(x) && typeof x.target_agent_id === "string", path);
  return useQuery({
    queryKey: agentKeys.agentGrants(scope, id),
    queryFn: async ({ signal }) => {
      if (qc.getQueryData<AccessGrants>(agentKeys.grants(scope, id))?.edition === "oss")
        return parse(await apiFetch<unknown>(path, { signal }));
      // query (the lab's fetchQuery), not ensureQueryData: after a write invalidates /grants, the derived lists must see the new rows.
      const grants = await qc.query({ ...grantsQuery(scope, id), staleTime: 5_000 });
      if (grants.edition === "oss") return parse(await apiFetch<unknown>(path, { signal }));
      // EE has no /grants/agents read; the Access tab names targets from the agents directory.
      return grants.agent_acl.map((tid) => ({ target_agent_id: tid, target_name: null }));
    },
    enabled,
  });
}

export function useSecrets(id: string, enabled: boolean) {
  const path = `/api/agents/${id}/secrets`;
  const parse = arrayOf<SecretName>((x) => isObj(x) && typeof x.name === "string", path);
  return useQuery({
    queryKey: agentKeys.secrets(useProductScope(), id),
    queryFn: async ({ signal }) => parse(await apiFetch<unknown>(path, { signal })),
    enabled,
  });
}

export function useAgentStats(id: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.stats(useProductScope(), id),
    queryFn: ({ signal }) =>
      apiData<z.infer<typeof agentStatsSchema>>(`${OBS}/agent/${id}/stats`, {
        signal,
        schema: agentStatsSchema,
      }),
    enabled,
    select: (d): AgentStats => d.project ?? d,
  });
}

export function useAgentResources(id: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.resources(useProductScope(), id),
    queryFn: ({ signal }) => apiData<AgentResources>(`${OBS}/agent/${id}/resources`, { signal }),
    enabled,
  });
}

/**
 * The Activity dot and the OSS crash card: newest ERROR lines. `since` must be RFC 3339 (a
 * relative value is silently dropped, observability/routes.rs:238) and the level filter runs
 * after the per-source limit, so this means "errors in the latest lines", not "last 24 h".
 */
export function useErrorLogs(id: string, enabled: boolean, now: () => number = Date.now) {
  return useQuery({
    queryKey: agentKeys.errorLogs(useProductScope(), id),
    queryFn: ({ signal }) =>
      apiFetch<LogLine[]>(
        withQuery(`${OBS}/agents/${id}/logs`, {
          level: "ERROR",
          limit: ERROR_LOG_LIMIT,
          since: new Date(now() - ERROR_LOG_WINDOW_MS).toISOString(),
        }),
        { signal },
      ),
    enabled,
    staleTime: ERROR_LOGS_STALE_MS,
  });
}

export function useAgentLogs(id: string, level: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.logs(useProductScope(), id, level),
    queryFn: ({ signal }) =>
      apiFetch<LogLine[]>(
        withQuery(`${OBS}/agents/${id}/logs`, { limit: LOG_LIMIT, level: level || undefined }),
        { signal },
      ),
    enabled,
  });
}

/** Proxied to the agent itself: only when the collapsible opens, and cached. */
export function useLiveCard(id: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.liveCard(useProductScope(), id),
    queryFn: ({ signal }) =>
      apiFetch<unknown>(`/api/agents/${id}/.well-known/agent-card.json`, { signal }),
    enabled,
    staleTime: LIVE_CARD_STALE_MS,
  });
}

/**
 * Recent sessions: the first page of session/list (no agent filter server-side), matched by
 * the raw agent name and created after the agent was (a deleted namesake's sessions drop).
 */
export function useAgentSessions(name: string, createdAt: string, enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.sessions(useProductScope(), name, createdAt),
    queryFn: ({ signal }) =>
      apiFetch<SessionListResponse>(
        withQuery(`${OBS}/session/list`, {
          start_time: createdAt || new Date(Date.now() - SESSIONS_FALLBACK_MS).toISOString(),
          limit: SESSIONS_SCAN,
          offset: 0,
        }),
        { signal },
      ),
    enabled: enabled && !!name,
    select: (r): SessionSummary[] => {
      const since = createdAt ? Date.parse(createdAt) : 0;
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- server data, read defensively whatever the generated type says
      return (r?.data?.sessions ?? []).filter(
        (s) => s.agent_id === name && (!s.start_time || Date.parse(s.start_time) >= since),
      );
    },
  });
}

type FinopsAgentRow = components["schemas"]["AgentFinopsRow"];

/** 24 h turns and est. cost per agent id (finops dashboard rows carry display names; join by id). */
export function useUsage24h(enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.usage24h(useProductScope()),
    queryFn: ({ signal }) =>
      apiData<{ agents: FinopsAgentRow[] }>(
        withQuery(`${OBS}/finops/dashboard`, { range: "24h", view: "agent" }),
        { signal, schema: usage24hSchema },
      ),
    enabled,
    staleTime: DETAIL_STALE_MS,
    select: (d) =>
      new Map(d.agents.map((r) => [r.agent_id, { turns: r.operations, cost: r.total_cost }])),
  });
}

/** Superusers only (users router is behind require_superuser). */
export function useUsers(enabled: boolean) {
  return useQuery({
    queryKey: agentKeys.users(useProductScope()),
    queryFn: ({ signal }) =>
      apiFetch<{ data: UserRow[] }>(withQuery("/api/users", { limit: USERS_LIMIT }), { signal }),
    enabled,
    staleTime: DIRECTORY_STALE_MS,
    // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- server data, read defensively whatever the generated type says
    select: (r) => new Map((r?.data ?? []).map((u) => [u.id, u.display_name || u.username])),
  });
}

export function useUserSearch(q: string) {
  const term = q.trim();
  return useQuery({
    queryKey: agentKeys.userSearch(useProductScope(), term),
    queryFn: ({ signal }) =>
      apiData<UserSearchResult[]>(withQuery("/api/search/users", { q: term }), { signal }),
    enabled: term.length >= USER_SEARCH_MIN,
    select: (rows) => rows.slice(0, USER_SEARCH_MAX),
  });
}

export function useCatalogTabs() {
  return useQuery({
    queryKey: agentKeys.settings(useProductScope()),
    queryFn: ({ signal }) =>
      apiFetch<{ catalog_tabs?: string | null } | { data?: { catalog_tabs?: string | null } }>(
        "/api/settings",
        { signal },
      ),
    staleTime: DIRECTORY_STALE_MS,
    select: (r): string[] => {
      /* eslint-disable @typescript-eslint/no-unnecessary-condition -- server data, read defensively whatever the generated type says */
      const raw =
        (r && "data" in r
          ? r.data?.catalog_tabs
          : (r as { catalog_tabs?: string | null })?.catalog_tabs) ?? "";
      /* eslint-enable @typescript-eslint/no-unnecessary-condition */
      return raw
        .split(",")
        .map((t) => t.trim())
        .filter(Boolean);
    },
  });
}

// ─── watch (truth checks) ───────────────────────────────────────────────────

/**
 * Per-agent lifecycle watches: client state that outlives a route (tab switches, remounts), so a
 * zustand store, not the query cache. Snapshots are replaced, never mutated. It ends with the
 * session: `onCacheReset` clears it on every cache clear or reset (sign-out, sign-in, a 401 expiry,
 * another account signing in), and deleting an agent drops its watch.
 */
const useWatchStore = create<Readonly<Record<string, Watch>>>(() => ({}));

const getWatch = (id: string): Watch | null => useWatchStore.getState()[id] ?? null;

function setWatch(id: string, w: Watch | null) {
  useWatchStore.setState((s) => {
    // eslint-disable-next-line @typescript-eslint/no-unused-vars -- the dropped watch
    const { [id]: _, ...rest } = s;
    return w ? { ...rest, [id]: w } : rest;
  }, true);
}

/** Drops every watch: every cache clear or reset runs it (onCacheReset). */
const clearWatches = () => {
  useWatchStore.setState({}, true);
};
onCacheReset(clearWatches);

export function useWatch(id: string) {
  const watch = useWatchStore((s) => s[id] ?? null);
  return {
    watch,
    watching: isWatching(watch),
    begin: (kind: WatchKind, buildId?: string) => {
      setWatch(id, startWatch(kind, Date.now(), buildId));
    },
    clear: () => {
      setWatch(id, null);
    },
  };
}

/** Whether any of these agents has a watch running (drives a list's polling). */
export function useAnyWatching(ids: readonly string[]): boolean {
  return useWatchStore((s) => ids.some((id) => isWatching(s[id])));
}

/** Store the next watch; when it just finished, refetch what the outcome changed (version, deployment). */
function applyWatch(qc: QueryClient, scope: Scope, id: string, w: Watch, next: Watch) {
  if (next === w) return;
  setWatch(id, next);
  if (next.outcome) {
    for (const key of [
      agentKeys.detail(scope, id),
      agentKeys.versions(scope, id),
      agentKeys.deployment(scope, id),
    ])
      void qc.invalidateQueries({ queryKey: key });
    // The directory's status feeds the Overview's fleet health: refresh it with the real outcome (/ship adversarial).
    void qc.invalidateQueries({ queryKey: agentKeys.directory(scope), exact: true });
  }
}

/**
 * Fold each fresh status into the active watch. `errorAt` is the status query's
 * `errorUpdatedAt`: a failed poll carries no reading, but still ends the watch at the cap.
 */
export function useWatchStep(
  id: string,
  display: DisplayStatus | undefined,
  updatedAt: number,
  errorAt = 0,
) {
  const qc = useQueryClient();
  const scope = useProductScope();
  useEffect(() => {
    if (!display) return;
    const w = getWatch(id);
    if (isWatching(w)) applyWatch(qc, scope, id, w, stepWatch(w, display, Date.now()));
  }, [qc, scope, id, display, updatedAt]);
  useEffect(() => {
    if (!errorAt) return;
    const w = getWatch(id);
    if (isWatching(w)) applyWatch(qc, scope, id, w, expireWatch(w, Date.now()));
  }, [qc, scope, id, errorAt]);
}

/**
 * A list's version of useWatchStep: steps every watched row from the page, so a row that
 * leaves the visible tab (or unmounts) keeps being checked until its watch ends.
 */
export function useWatchesStep(
  rows: readonly { id: string; display: DisplayStatus }[],
  updatedAt: number,
  errorAt: number,
) {
  const qc = useQueryClient();
  const scope = useProductScope();
  useEffect(() => {
    for (const r of rows) {
      const w = getWatch(r.id);
      if (isWatching(w)) applyWatch(qc, scope, r.id, w, stepWatch(w, r.display, Date.now()));
    }
    // rows is rebuilt from the same data as updatedAt; stepping on updatedAt alone avoids double steps.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [qc, scope, updatedAt]);
  useEffect(() => {
    if (!errorAt) return;
    for (const r of rows) {
      const w = getWatch(r.id);
      if (isWatching(w)) applyWatch(qc, scope, r.id, w, expireWatch(w, Date.now()));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [qc, scope, errorAt]);
}

// ─── mutations ──────────────────────────────────────────────────────────────

const json = (body: unknown): RequestInit => ({
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(body),
});

/**
 * Only what a lifecycle action, roll back or edit changes: the status lists and this agent's
 * detail, deployment, versions and the directory (fleet health reads its status). Usage, logs and sessions stay put.
 * cancelRefetch (default) drops an in-flight poll, so a stale answer can't overwrite the new state.
 */
function invalidateAgent(qc: QueryClient, scope: Scope, id: string) {
  for (const key of [
    k(scope, "owned"),
    k(scope, "status-page"),
    agentKeys.detail(scope, id),
    agentKeys.deployment(scope, id),
    agentKeys.versions(scope, id),
  ]) {
    void qc.invalidateQueries({ queryKey: key });
  }
  // The directory carries status for the Overview's fleet health: a restart must not leave it saying "crashed".
  void qc.invalidateQueries({ queryKey: agentKeys.directory(scope), exact: true });
}

/** Lists that show names (a rename or delete changes them). Inactive ones just go stale. */
function invalidateNameLists(qc: QueryClient, scope: Scope) {
  void qc.invalidateQueries({ queryKey: agentKeys.directory(scope), exact: true });
  void qc.invalidateQueries({ queryKey: agentKeys.catalog(scope) });
}

export function useLifecycle(id: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    mutationFn: (action: "restart" | "stop" | "start") =>
      apiFetch<unknown>(`/api/containers/${id}/${action}`, { method: "POST" }),
    onSettled: () => {
      invalidateAgent(qc, scope, id);
    },
  });
}

export function useRollback(id: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    mutationFn: (v: { target_version: string; reason?: string }) =>
      apiFetch<RollbackResponse>(`/api/agents/${id}/rollback`, json(v)),
    onSettled: () => {
      invalidateAgent(qc, scope, id);
    },
  });
}

export function useDeleteAgent(id: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    mutationFn: () => apiFetch<DeletedAgent>(`/api/agents/${id}`, { method: "DELETE" }),
    onSuccess: () => {
      // Drop everything about the deleted agent, so nothing refetches a 404, and its watch.
      qc.removeQueries({
        queryKey: agentKeys.all(scope),
        predicate: (q) => q.queryKey[PART(2)] === id,
      });
      setWatch(id, null);
      for (const key of [k(scope, "owned"), k(scope, "status-page")])
        void qc.invalidateQueries({ queryKey: key });
      invalidateNameLists(qc, scope);
    },
  });
}

/** `PUT /api/agents/{id}` (models.rs `UpdateAgentRequest`): the fields the Settings tab writes. */
export interface AgentUpdate {
  display_name?: string;
  description?: string;
  metadata?: Record<string, unknown>;
  compress_enabled?: boolean;
  minimal_code_enabled?: boolean;
}

export function useUpdateAgent(id: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    // Every field is COALESCEd server-side, so a body carries only what it changes (catalog/routes.rs update).
    mutationFn: (v: AgentUpdate) =>
      apiFetch<Agent>(`/api/agents/${id}`, { ...json(v), method: "PUT" }),
    onSettled: () => {
      invalidateAgent(qc, scope, id);
      invalidateNameLists(qc, scope);
      // Pending until the detail is re-read, so a Settings switch never flashes back to its old state.
      return qc.refetchQueries({ queryKey: agentKeys.detail(scope, id) }, { cancelRefetch: false });
    },
  });
}

/**
 * The Token optimization switch (plans/feat-context-optimization.md eng E1): `PUT /api/agents/{id}` with only
 * `compress_enabled` (the update is COALESCE per field, catalog/routes.rs `update`).
 */
export function useSetCompression(id: string, onFailed: (err: Error) => void) {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    mutationFn: (on: boolean) =>
      apiFetch<Agent>(`/api/agents/${id}`, { ...json({ compress_enabled: on }), method: "PUT" }),
    // Hook-level, so a failure is reported even after the page that flipped it unmounted (the flip was optimistic).
    onError: onFailed,
    // The detail and the owned lists (the Optimization page's compression line) are refetched before the switch lets go
    // of its optimistic value. The directory and catalog follow as after any agent edit (Needs attention's other owners
    // line reads the directory), and both savings reads (By agent's On/Off, the Mechanisms counts) are re-read.
    onSettled: () => {
      invalidateNameLists(qc, scope);
      void qc.invalidateQueries({ queryKey: productKey(scope, "tokenops", "savings") });
      void qc.invalidateQueries({ queryKey: productKey(scope, "tokenops", "context-savings") });
      return Promise.all([
        qc.invalidateQueries({ queryKey: agentKeys.detail(scope, id) }),
        qc.invalidateQueries({ queryKey: k(scope, "owned") }),
        qc.invalidateQueries({ queryKey: k(scope, "status-page") }),
      ]);
    },
  });
}

/** Bulk turn-ons in flight; a cache reset (sign-out, another account) stops their queued writes. */
const bulkBatches = new Set<AbortController>();
onCacheReset(() => {
  for (const stop of bulkBatches) stop.abort();
  bulkBatches.clear();
});

/** At most this many Token optimization writes in flight for a bulk turn-on (plans/feat-optimization-page.md R2F). */
const BULK_WRITES = 4;

interface TurnOnResult {
  done: string[];
  failed: { id: string; error: unknown }[];
}

/**
 * The Optimization page's bulk turn-on (plans/feat-optimization-page.md R2F, eng C7): `PUT /api/agents/{id}
 * {compress_enabled: true}` for each agent, 4 at a time, as one mutation. Each write settles on its own (a failure is
 * returned, not thrown, so the rest still run); a 401 is rethrown for the app's expiry path. Once per batch it re-reads
 * every agents query (the restart watches are cache-only state and stay) and both savings reads, so the strip, By agent
 * and the Mechanisms counts change together.
 */
export function useTurnOnCompression() {
  const qc = useQueryClient();
  const scope = useProductScope();
  return useMutation({
    mutationFn: async (v: {
      ids: readonly string[];
      onProgress?: (settled: number) => void;
    }): Promise<TurnOnResult> => {
      const run = createLimiter(BULK_WRITES);
      // A confirmed batch finishes even if the page goes away (each write is one PUT), but never outlives its session:
      // the first 401, a sign-out (the cache reset) or another sign-in stops the writes not yet started, so a queued PUT
      // can't run with someone else's cookie (review: adversarial, Codex P1).
      const stop = new AbortController();
      const signal = stop.signal;
      const generation = signInGeneration();
      bulkBatches.add(stop);
      let settled = 0;
      const out = await Promise.all(
        v.ids.map((id) =>
          run(signal, () => {
            if (isSigningOut() || isSignedOutLocally() || signInGeneration() !== generation) {
              stop.abort();
              throw new DOMException("The session changed", "AbortError");
            }
            return apiFetch<Agent>(`/api/agents/${id}`, {
              ...json({ compress_enabled: true }),
              method: "PUT",
              signal,
            });
          })
            .then(
              () => ({ id, error: null as unknown }),
              (error: unknown) => {
                if (isUnauthorized(error)) stop.abort();
                return { id, error };
              },
            )
            .finally(() => v.onProgress?.(++settled)),
        ),
      );
      bulkBatches.delete(stop);
      const expired = out.find((o) => o.error !== null && isUnauthorized(o.error));
      if (expired) throw expired.error;
      return {
        done: out.filter((o) => o.error === null).map((o) => o.id),
        failed: out.filter((o) => o.error !== null),
      };
    },
    onSettled: () =>
      Promise.all([
        qc.invalidateQueries({
          queryKey: agentKeys.all(scope),
          predicate: (q) => q.queryKey[PART(1)] !== "watch",
        }),
        qc.invalidateQueries({ queryKey: productKey(scope, "tokenops", "context-savings") }),
        // The Mechanisms block's agents-on counts (/finops/savings coverage).
        qc.invalidateQueries({ queryKey: productKey(scope, "tokenops", "savings") }),
      ]),
  });
}

export function useSecretMutations(id: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  // Returned, so a write stays pending until the list is re-read (the Self-review switch reads it).
  const done = () => qc.invalidateQueries({ queryKey: agentKeys.secrets(scope, id) });
  return {
    // gcTime 0 + the caller's reset(): the value must not stay in the mutation cache as `variables`.
    set: useMutation({
      mutationFn: (v: { name: string; value: string }) =>
        apiFetch<null>(`/api/agents/${id}/secrets`, json(v)),
      onSettled: done,
      gcTime: 0,
    }),
    remove: useMutation({
      mutationFn: (name: string) =>
        apiFetch<null>(`/api/agents/${id}/secrets/${encodeURIComponent(name)}`, {
          method: "DELETE",
        }),
      onSettled: done,
    }),
  };
}

export function useGrantMutations(id: string) {
  const qc = useQueryClient();
  const scope = useProductScope();
  const done = () => {
    for (const key of [
      agentKeys.grants(scope, id),
      agentKeys.userGrants(scope, id),
      agentKeys.agentGrants(scope, id),
    ])
      void qc.invalidateQueries({ queryKey: key });
  };
  // The Access tab has loaded /grants before any write, so the edition is known (grants.ts).
  const edition = () => qc.getQueryData<AccessGrants>(agentKeys.grants(scope, id))?.edition;
  const grantPost = (w: { path: string; body?: unknown }) =>
    apiFetch<null>(w.path, w.body === undefined ? { method: "POST" } : json(w.body));
  return {
    setPublic: useMutation({
      mutationFn: (on: boolean) =>
        apiFetch<null>(`/api/agents/${id}/grants/public`, { method: on ? "POST" : "DELETE" }),
      onSettled: done,
    }),
    addUser: useMutation({
      mutationFn: (userId: string) => grantPost(grantWrite(edition(), id, "users", userId)),
      onSettled: done,
    }),
    removeUser: useMutation({
      mutationFn: (userId: string) =>
        apiFetch<null>(`/api/agents/${id}/grants/users/${userId}`, { method: "DELETE" }),
      onSettled: done,
    }),
    addAgent: useMutation({
      mutationFn: (agentId: string) => grantPost(grantWrite(edition(), id, "agents", agentId)),
      onSettled: done,
    }),
    removeAgent: useMutation({
      mutationFn: (agentId: string) =>
        apiFetch<null>(`/api/agents/${id}/grants/agents/${agentId}`, { method: "DELETE" }),
      onSettled: done,
    }),
  };
}
