// Ported from nasiko-ui-lab@1b99bdda common/src/app/edition.ts. Local edits: `LayerNavItem`/`AnyNavItem` are declared
// in shell/nav.ts and re-exported here (the lab's two type-only imports formed a cycle the dependency checker rejects;
// same types, same name for callers); the `shellSplit` slot (development 66a4c64d0, 3794209ec: the EE assistant panel);
// the `tours` slot (dd9fe0f64); the OSS defaults are the only edition here, the slot types stay so `useSlots()` keeps
// its shape. Development (e76020c15, 1c28b0bb9): the `homeOrgScope` slot, `budgetsOrgRole` and the `budgets` surface
// (`NO_BUDGETS` in an OSS build).
/**
 * How an edition builds on the core (docs/lab-vs-react-migration-review.md §10.4). Pure TypeScript, no React:
 * the glue is edition-context.tsx.
 *
 * - The EDITION is fixed at build time: each edition's `src/edition.ts` names its id and its layers, and nothing
 *   checks it at run time except `useEdition()`. An OSS build contains no EE code at all (the build's leak guard).
 * - A layer changes the core only through `slots` (each has an OSS fallback in the core) and `nav` patches
 *   relative to core items; it never redeclares the item list, so a new core item shows in every edition.
 * - Capability (what this server can do right now) is still probed at run time, and only for features both
 *   editions have (the harness usage endpoint's bare vs coded 404, crash-guardian fields). A probe never stands in
 *   for the edition.
 */
import type { ComponentType, ReactNode } from "react";
import type { AnyNavItem, LayerNavItem, NavGroupId } from "../shell/nav";
import type { TourStep } from "../tour";

export type { AnyNavItem } from "../shell/nav";

export type EditionId = "oss" | "ee" | "mt";

/** What the Harnesses page may show above the individual view. OSS: only the viewer's own usage. */
interface HarnessOrgScope {
  /** The org-level page (units, landing rules, org/team rows, the breadcrumb root). */
  Page: ComponentType;
}

/** A query a layer hands the core to run (from a lazy chunk, so the layer's entry code needs no query hooks). */
interface SlotQuery<T> {
  queryKey: readonly unknown[];
  queryFn: (ctx: { signal: AbortSignal }) => Promise<T>;
  staleTime?: number;
}

/** A level the home's scope control adds after Yours and Fleet (EE: an org unit, with its subtree). */
export interface HomeLevel {
  /** The level's id, written to `?scope=unit:<id>`. */
  id: string;
  label: string;
  /** Its ancestors' names and its own ("Engineering › Platform"): the accessible name, so same-named units differ. */
  path: string;
  /** Nesting under its visible parent, for the picker's indent (0 = a root). */
  depth: number;
}

/**
 * The home's org levels (plans/feat-agent-centric.md PR2c; eng "homeOrgScope defines { levels, filterRows, rollup }").
 * OSS: Yours and Fleet only.
 * - `levels`: the units the viewer may pick (the server already limits them: a manager's led subtrees, nothing for a
 *   member).
 * - `owners`: the users whose agents a level shows (its members, subtree included); the home keeps the rows they own.
 *   The roll-up stays the sum of those rows: there is no team budget to show (R-L10 has owner and agent budgets only).
 */
export interface HomeOrgScope<L = unknown, M = unknown> {
  levels: { query: SlotQuery<L>; list: (data: L) => readonly HomeLevel[] };
  owners: {
    query: (id: string) => SlotQuery<M>;
    /** The owners, or null when the server wouldn't say (`{"available": false}`): the level reads as not visible. */
    ids: (data: M) => ReadonlySet<string> | null;
    /** Owners' names, so a manager's unit groups rows by person even without the user list. */
    names: (data: M) => ReadonlyMap<string, string>;
  };
}

export const homeOrgScope = <L, M>(s: HomeOrgScope<L, M>) => s as unknown as HomeOrgScope;

/** Who else an agent can be shared with, besides users (EE: org units). */
export interface GranteeKind {
  key: string;
  label: string;
  /** The Access tab section for this kind of grantee. */
  Section: ComponentType<{ agentId: string }>;
}

/** Who else an MCP server can be shared with, besides users (EE: org units). */
interface ConnectorGranteeKind {
  key: string;
  label: string;
  /** The server page's Access tab section for this kind of grantee. */
  Section: ComponentType<{ connectorId: string }>;
}

/** A section a layer adds to Settings' in-page nav (EE: Single sign-on), at `/settings?section=<key>`. */
export interface SettingsSection {
  key: string;
  label: string;
  /** Which nav group it sits in; Security sections come before Secrets. */
  group: "workspace" | "security";
  /** The row it follows in its group (e.g. `general`); the end of the group when absent. */
  after?: string;
  /** Admin-only, like the core's workspace sections. It draws its own page header (its words load with it). */
  Section: ComponentType;
}

/**
 * A layer's own personal (non-admin) settings row, e.g. EE's memory recall consent. Unlike `SettingsSection` — which
 * renders at `/settings?section=<key>`, a page the core keeps for superusers — this is a plain nav link to the layer's
 * own route, so every user, admin or not, can reach it. It sits in the Security group, after Secrets.
 */
export interface PersonalSettingsLink {
  key: string;
  label: string;
  /** The layer's own route, e.g. `/settings/memory` — not `/settings?section=`. */
  to: string;
}

/**
 * Sign-in methods a layer adds under /login's password form (EE: single sign-on). A method leaves the app for its
 * provider: it calls `onStart` first, then sends the browser away with `returnTo` as the path to come back to.
 * `/login` finishes that sign-in itself; a failed one comes back with `#error=<code>`, handed over as `error`.
 */
export interface LoginMethodsProps {
  returnTo: string;
  onStart: () => void;
  error?: string | undefined;
}

/** The org-unit filter a layer can add to TokenOps (planned in EE). */
interface OrgUnitFilterProps {
  value?: string;
  onChange: (orgUnit: string | undefined) => void;
}

/**
 * Budgets are an **Enterprise** feature end to end (docs/requirements/budgets.md, user decision 2026-10-14): the
 * feature, its queries, its mocks and its pages live in the Enterprise layer, and an OSS build contains
 * none of it. What stays in the core is this contract — the shapes the core's own surfaces render when a layer
 * fills them, exactly as `HomeLevel` is the contract for EE's org units.
 *
 * Every member is a hook or a component, and the OSS answer (`NO_BUDGETS`) is a real implementation returning
 * "off", not `null`: the core calls these unconditionally, so a nullable slot would make them conditional hooks.
 */
export type BudgetCrossing = { kind: "reached" } | { kind: "crosses"; date: string };

/** One budget's use this period, as the home's table cell and the hub read it. */
export interface RowBudget {
  used: number;
  limit: number;
  /** When the period resets (ISO), for the month name. */
  resetsAt: string;
  /** Calls with no price this period: counted as $0, so use is a floor. */
  unpriced: boolean;
  crossing: BudgetCrossing | null;
}

/** A budget that needs the viewer, as a Needs you row. */
export interface BudgetItem {
  id: string;
  label: string;
  /** An agent budget's agent: the row opens that agent's Budget section on its hub. */
  agentId: string | null;
  crossed: number | null;
  state: "warning" | "exceeded";
  /** Dollars over its limit, when it is. */
  over: number | null;
  /** Its calls are refused right now. */
  stopped: boolean;
  /** It stops calls at its limit (else it alerts only). */
  stops: boolean;
  /** The day it is projected to reach its limit, before the reset. */
  runsOutOn: string | null;
  resetsAt: string | null;
  /** For the detail line: what period and population it counts. */
  period: "month" | "quarter" | "session";
  covers: "routed_calls" | "all_priced";
}

/** Where budgets stand on this server: `off` covers both "no layer" and "this server has none" (a bare 404). */
export type BudgetsState = "off" | "loading" | "failed" | "stale" | "ok";

export interface HomeBudgets {
  state: BudgetsState;
  /** Agent budgets by agent id; null until both reads answered. */
  byAgent: ReadonlyMap<string, RowBudget> | null;
  /** The viewer's own monthly budget, when they set one. */
  own: RowBudget | null;
  updatedAt: number;
  /** Mock data (a preview of the contract): the page labels it so. */
  preview: boolean;
}

/** What the Overview's fleet-health rating needs from budgets: a stopped or exceeded budget is an agent's reason. */
export interface OverviewBudgets {
  byAgent: ReadonlyMap<string, { stopped: boolean; exceeded: boolean }>;
  /** Every budget read has answered (or is absent): Needs you may say "nothing needs you". */
  settled: boolean;
  /** A read failed: cost reasons are incomplete, never an all-clear. */
  failed: boolean;
  /** Re-run the budget reads behind this rating. */
  retry: () => void;
}

/** A loading / absent / failed / present answer, the shape Needs you and the hub both take. */
export type BudgetSource<T> =
  | { state: "loading" }
  | { state: "absent" }
  | { state: "failed" }
  | { state: "ok"; value: T; incomplete?: boolean };

/** The fields the hub's Budget section needs; the core passes them rather than its own `AgentView`. */
export interface HubBudgetAgent {
  id: string;
  name: string;
  ownerId: string | null;
  isHarness: boolean;
}

export interface BudgetsSurface {
  /** The home's budget column and the viewer's own budget. */
  useHomeBudgets: (enabled: boolean, meSub: string) => HomeBudgets;
  /** The Overview's rating inputs. */
  useOverviewBudgets: (enabled: boolean) => OverviewBudgets;
  /** Needs you's budget rows. */
  useBudgetNeeds: (
    me: { sub: string; is_superuser: boolean },
    enabled: boolean,
    agentLabel: (agentId: string) => string | undefined,
    shownAsAgent: (agentId: string) => boolean,
  ) => {
    value: BudgetSource<BudgetItem[]>;
    retry: () => void;
    /** When the rows were read, for Needs you's "checked at" line; 0 when nothing was read. */
    stamp: number;
  };
  /** One agent's budget for the hub's Needs you (`null` level: the budget needs nothing). */
  useAgentBudgetNeed: (
    agent: { id: string; ownerId: string; isHarness: boolean },
    me: { sub: string; is_superuser: boolean } | undefined,
  ) => {
    state: BudgetSource<{ level: "action" | "watch" | null; sentence: string }>;
    retry: () => void;
  };
  /**
   * TokenOps' "Budgets →" link beside Month-end at pace. A component, not a boolean: the core cannot type a link
   * to `/budgets`, a route only the EE build has.
   */
  OpenLink: ComponentType | null;
  /** Needs you's "Open budgets" action on a budget row that names no agent. Same reason as `OpenLink`. */
  NeedsLink: ComponentType<{ label: string; ariaLabel: string }> | null;
  /**
   * Sidebar paths to hide right now. OSS has no budgets item at all, so `[]`; EE hides `/budgets` while its
   * availability probe is out, fails, or says this server has none. The core never names the path.
   */
  useNavHidden: () => readonly string[];
  /**
   * Home's Budget burn panel, which leads the chart band. A hook, not a component: the band also needs to know the
   * panel is still loading, so it can hold its row and withhold Over time rather than swap mid-look.
   */
  useHomePanel: (args: {
    /** `fleet`, `yours`, or `unit:<id>`; null while the scope is still resolving. */
    scope: string | null;
    meSub: string;
    isSuperuser: boolean;
    /** The picked unit's name, when the scope is a unit. */
    unitLabel: string | null;
  }) => { node: ReactNode; pending: boolean };
  /** The agent hub's Budget section. */
  HubSection: ComponentType<{ agent: HubBudgetAgent }> | null;
  /**
   * The session ledger's per-turn budgets (the AH-8 preview). The core owns the ledger's sticky bar and table, so
   * the layer hands back the three pieces they need rather than rendering them: the per-turn running share for the
   * table's column, the headline for the bar, and the panel below.
   */
  useLedgerBudgets: (
    sessionId: string,
    enabled: boolean,
  ) => { shares: Map<string, number> | null; headline: ReactNode; panel: ReactNode };
  /** The session ledger's budget. */
  LedgerSection: ComponentType<{
    sessionId: string;
    title: string;
    /** The session is one of the viewer's own chats. */
    ownSession: boolean;
  }> | null;
  /** Whether the LLM router page lists a Budgets tab (the layer's own availability probe). */
  useRouterTab: () => boolean;
  /** The Budgets tab's content, rendered inside the router page's `TabsContent`. */
  RouterTab: ComponentType<{
    agents: readonly { id: string; name: string }[];
    agentName: (agentId: string) => string | undefined;
    announce: (message: string) => void;
    /** The page's other reads have settled, so `#router-budgets` can scroll to its final position. */
    pageReady: boolean;
  }> | null;
}

const NO_BUDGET_SOURCE = { state: "absent" } as const;
const NO_PATHS: readonly string[] = [];
const NO_PANEL = { node: null, pending: false };
const NO_LEDGER_BUDGETS = { shares: null, headline: null, panel: null };
const NO_RETRY = () => {};
/** The OSS answer: budgets do not exist in this build, so every surface is off and nothing is ever read. */
export const NO_BUDGETS: BudgetsSurface = {
  useHomeBudgets: () => ({ state: "off", byAgent: null, own: null, updatedAt: 0, preview: false }),
  useOverviewBudgets: () => ({
    byAgent: new Map(),
    settled: true,
    failed: false,
    retry: NO_RETRY,
  }),
  useBudgetNeeds: () => ({ value: NO_BUDGET_SOURCE, retry: NO_RETRY, stamp: 0 }),
  useAgentBudgetNeed: () => ({ state: NO_BUDGET_SOURCE, retry: NO_RETRY }),
  OpenLink: null,
  NeedsLink: null,
  useNavHidden: () => NO_PATHS,
  HubSection: null,
  LedgerSection: null,
  useHomePanel: () => NO_PANEL,
  useLedgerBudgets: () => NO_LEDGER_BUDGETS,
  useRouterTab: () => false,
  RouterTab: null,
};

/** Every slot has an OSS fallback in the core; a slot is added in the PR whose layer first fills it. */
export interface Slots {
  harnessOrgScope: HarnessOrgScope | null;
  homeOrgScope: HomeOrgScope | null;
  granteeKinds: GranteeKind[];
  connectorGranteeKinds: ConnectorGranteeKind[];
  settingsSections: SettingsSection[];
  personalSettingsLinks: PersonalSettingsLink[];
  loginMethods: ComponentType<LoginMethodsProps> | null;
  tokenopsOrgUnitFilter: ComponentType<OrgUnitFilterProps> | null;
  topbarStart: ComponentType | null;
  shellEnd: ComponentType<{ path: string }> | null;
  /**
   * Wraps the page in a split with a panel of its own (EE: its assistant). While the panel shows beside the
   * page, it sets `--shell-split-end` (its width) on an ancestor of `children`, so fixed page layers (`PageLoader`)
   * stop short of it.
   */
  shellSplit: ComponentType<{ path: string; children: ReactNode }> | null;
  /**
   * Rows a layer adds to /optimization's Workspace footer, after the core's tiers row (EE: Organization policy;
   * plans/feat-optimization-page.md R5B). Superusers only, like the footer. Each renders a `WorkspaceRow` or nothing.
   */
  optimizationWorkspace: ComponentType | null;
  /** Sidebar items to hide for this viewer (EE: Access control below `can_read_org`). */
  navHidden: NavHidden | null;
  /** Tours of the layer's own pages (legacy ui/ on development, dd9fe0f64). */
  tours: LayerTour[];
  /**
   * The viewer's org role for budgets (plans/feat-budget-burn.md B6; budgets.md §5). OSS: none, the superuser flag
   * alone. EE: an org admin reads and writes like a superuser, finance reads everything and writes nothing, a unit lead
   * reads (and, while the org allows it, writes) the units they lead; the units themselves are `homeOrgScope`'s levels.
   */
  budgetsOrgRole: BudgetsOrgRole | null;
  /** Budgets (Enterprise only). `NO_BUDGETS` in an OSS build: every surface off, nothing read. */
  budgets: BudgetsSurface;
}

export interface BudgetsOrgRoleOf {
  /** The account the org context speaks for (owner budgets are this id's). */
  id: string;
  admin: boolean;
  finance: boolean;
  /** Units the viewer leads (their sub-units come with them). */
  leads: readonly string[];
}

/** One query and the role it says. Build it with `budgetsOrgRole()`, which pairs the two. */
export interface BudgetsOrgRole<T = unknown> {
  query: SlotQuery<T>;
  role: (data: T) => BudgetsOrgRoleOf;
}

export const budgetsOrgRole = <T>(r: BudgetsOrgRole<T>) => r as unknown as BudgetsOrgRole;

/**
 * One query and the items to hide given its data (`undefined` while it loads). The shell runs the query from its lazy
 * route chunk, so a layer's entry code needs no query hooks (they'd cost the EE shell ~3 KB gz). Build it with
 * `navHidden()`, which pairs the query's data type with `hidden`.
 */
export interface NavHidden<T = unknown> {
  query: {
    queryKey: readonly unknown[];
    queryFn: (ctx: { signal: AbortSignal }) => Promise<T>;
    staleTime?: number;
  };
  hidden: (data: T | undefined) => readonly string[];
}

export const navHidden = <T>(n: NavHidden<T>) => n as NavHidden;

/**
 * The product tour of a layer's own pages (EE: Access control). Matched after the core's pages; its steps load only
 * when it opens, so a layer's tour text stays out of its shell.
 */
export interface LayerTour {
  /** Remembered as seen under this id: unique across the core's tours (`features/tour/logic.ts`) and every layer's. */
  id: string;
  /** The pages it covers (pathname). */
  match: RegExp;
  steps: () => Promise<readonly TourStep[]>;
}

/** The core's answer for every slot: an OSS build renders exactly this. */
export const OSS_SLOTS: Slots = {
  harnessOrgScope: null,
  homeOrgScope: null,
  granteeKinds: [],
  connectorGranteeKinds: [],
  settingsSections: [],
  personalSettingsLinks: [],
  loginMethods: null,
  tokenopsOrgUnitFilter: null,
  topbarStart: null,
  shellEnd: null,
  shellSplit: null,
  optimizationWorkspace: null,
  navHidden: null,
  tours: [],
  budgetsOrgRole: null,
  budgets: NO_BUDGETS,
};

type NavPatch =
  | { op: "insert"; after: string; items: LayerNavItem[] }
  | { op: "hide"; urls: string[] }
  | { op: "append"; group: NavGroupId; items: LayerNavItem[] };

export interface EditionLayer {
  id: Exclude<EditionId, "oss">;
  slots?: Partial<Slots>;
  nav?: NavPatch[];
}

export interface Edition {
  id: EditionId;
  layers: readonly EditionLayer[];
}

export const OSS: Edition = { id: "oss", layers: [] };

/** Scalar slots: the upper layer wins. Array slots: concatenated, lowest layer first. */
export function resolveSlots(layers: readonly EditionLayer[]): Slots {
  const out: Slots = {
    ...OSS_SLOTS,
    granteeKinds: [],
    connectorGranteeKinds: [],
    settingsSections: [],
    personalSettingsLinks: [],
    tours: [],
  };
  for (const layer of layers) {
    const s = layer.slots ?? {};
    if (s.granteeKinds) out.granteeKinds = [...out.granteeKinds, ...s.granteeKinds];
    if (s.connectorGranteeKinds)
      out.connectorGranteeKinds = [...out.connectorGranteeKinds, ...s.connectorGranteeKinds];
    if (s.settingsSections) out.settingsSections = [...out.settingsSections, ...s.settingsSections];
    if (s.personalSettingsLinks)
      out.personalSettingsLinks = [...out.personalSettingsLinks, ...s.personalSettingsLinks];
    if (s.tours) out.tours = [...out.tours, ...s.tours];
    if (s.harnessOrgScope !== undefined) out.harnessOrgScope = s.harnessOrgScope;
    if (s.homeOrgScope !== undefined) out.homeOrgScope = s.homeOrgScope;
    if (s.loginMethods !== undefined) out.loginMethods = s.loginMethods;
    if (s.tokenopsOrgUnitFilter !== undefined) out.tokenopsOrgUnitFilter = s.tokenopsOrgUnitFilter;
    if (s.topbarStart !== undefined) out.topbarStart = s.topbarStart;
    if (s.shellEnd !== undefined) out.shellEnd = s.shellEnd;
    if (s.shellSplit !== undefined) out.shellSplit = s.shellSplit;
    if (s.optimizationWorkspace !== undefined) out.optimizationWorkspace = s.optimizationWorkspace;
    if (s.navHidden !== undefined) out.navHidden = s.navHidden;
    if (s.budgetsOrgRole !== undefined) out.budgetsOrgRole = s.budgetsOrgRole;
    if (s.budgets !== undefined) out.budgets = s.budgets;
  }
  return out;
}

/** The sidebar items after every layer's patches, in layer order. An unknown `after` appends to that item's group. */
export function applyNav(
  base: readonly AnyNavItem[],
  layers: readonly EditionLayer[],
): AnyNavItem[] {
  let items = [...base];
  for (const patch of layers.flatMap((l) => l.nav ?? [])) {
    if (patch.op === "hide") items = items.filter((i) => !patch.urls.includes(i.to));
    else if (patch.op === "append")
      items = [...items, ...patch.items.map((i) => ({ ...i, group: patch.group }))];
    else {
      const at = items.findIndex((i) => i.to === patch.after);
      items =
        at < 0
          ? [...items, ...patch.items]
          : [...items.slice(0, at + 1), ...patch.items, ...items.slice(at + 1)];
    }
  }
  return items;
}
