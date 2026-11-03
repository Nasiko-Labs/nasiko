/**
 * The estate: everything the Overview's map draws, in one read (docs/requirements/overview.md OV-1..OV-3). None of
 * these routes exists yet, so the wire types are zod schemas: `z.looseObject`, only the fields the UI reads.
 *
 * - OV-1 `GET /api/overview/estate?range=7d|30d|90d`: the nodes (groups, people, workflows, routers, providers,
 *   models, agents, tools, coding harnesses), the flows between the map's cards, and the problems, cause chains and
 *   opportunities computed over them. `mine` lists what the viewer owns or touches, so Mine and Org are one read.
 * - OV-2 `PATCH /api/overview/problems/{id}`: who is on a problem, a snooze, or "handled". Answers the problem.
 * - OV-3 `GET|PUT /api/me/overview-pins`: the lens cards pinned to the Overview, in order.
 *
 * A bare 404 from OV-1 means the server has none of them: the page builds a smaller map from the reads it already
 * makes (`fallback.ts`) and leaves out what only OV-2 and OV-3 can back.
 */
import { z } from 'zod'

/** The kinds of problem. Colour means the kind, nothing else (errors red, slow orange, cost purple). */
export const PROBLEM_KINDS = ['errors', 'slow', 'cost'] as const
export type ProblemKind = (typeof PROBLEM_KINDS)[number]
export const TIERS = ['broken', 'worse'] as const
export type Tier = (typeof TIERS)[number]

/** The map's six cards; flows run between them. */
export const PANELS = ['people', 'workflows', 'models', 'agents', 'tools', 'harnesses'] as const
export type PanelId = (typeof PANELS)[number]

/** Where a problem's, chain's or opportunity's one action goes: a page the UI knows, never a raw URL. */
const targetSchema = z.looseObject({
  page: z.enum([
    'agent',
    'agents',
    'builds',
    'sessions',
    'tokenops',
    'optimization',
    'router',
    'mcp',
    'connector',
    'workflow',
    'harnesses',
    'chat',
  ]),
  id: z.string().nullish(),
  /** Search params the page understands (`lane=failing`, `day=…`). */
  query: z.record(z.string(), z.string()).nullish(),
})
export type Target = z.infer<typeof targetSchema>

const budget = z.looseObject({
  limit_usd: z.number(),
  spent_usd: z.number(),
  projected_usd: z.number(),
  /** Coding-harness spend: it skips the routers, so a cap can only alert on it. */
  harness_usd: z.number(),
  hard: z.boolean(),
})

const groupSchema = z.looseObject({ id: z.string(), name: z.string(), budget: budget.nullable() })

export const SEGMENTS = ['value', 'expensive', 'stuck', 'light'] as const
export type Segment = (typeof SEGMENTS)[number]

const personSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  group_id: z.string().nullable(),
  spend_usd: z.number(),
  sessions: z.number(),
  success_pct: z.number().nullable(),
  avg_context_tokens: z.number().nullable(),
  segment: z.enum(SEGMENTS),
  /**
   * Proposed (plans/feat-village-overview.md V-6): what the person is working on right now (their latest session in
   * the last few minutes), so the village stands them beside it. Absent or null: they stand with their team.
   */
  working_on: z
    .looseObject({ kind: z.enum(['team', 'agent', 'router', 'project']), id: z.string() })
    .nullish(),
})

const stepSchema = z.looseObject({
  kind: z.enum(['agent', 'tool', 'approval', 'step']),
  ref: z.string().nullable(),
  label: z.string(),
})
const workflowSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  owner_id: z.string().nullable(),
  runs: z.number(),
  steps: z.array(stepSchema),
  /** Proposed (V-8): the project the workflow belongs to. */
  project_id: z.string().nullish(),
})
/** Proposed (V-8): a project, the team it belongs to (null: org-wide) and its workflows. */
const projectSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  owner_id: z.string().nullable(),
  group_id: z.string().nullable(),
  workflow_ids: z.array(z.string()),
})

const routerSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  /** Traffic that skips every router (coding harnesses go straight to a provider). */
  direct: z.boolean(),
  note: z.string().nullable(),
})
const providerSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  /** A brand mark the UI knows (`anthropic`, `openai`, …), else a monogram. */
  brand: z.string().nullable(),
  share_pct: z.number(),
  /**
   * Proposed (V-7): the org's coding-harness use of this provider is billed on the same account as its product use,
   * so the village joins the two mines across the causeway.
   */
  billed_with_harnesses: z.boolean().nullish(),
})
const modelSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  /** A one-word label under the model's square (`Sonnet 4`). */
  short: z.string().nullish(),
  provider_id: z.string(),
  spend_usd: z.number(),
  calls: z.number(),
  p95_ms: z.number().nullable(),
})
const agentSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  owner_id: z.string().nullable(),
  router_id: z.string().nullable(),
  model_ids: z.array(z.string()),
  tool_ids: z.array(z.string()),
  spend_usd: z.number(),
  runs: z.number(),
  success_pct: z.number().nullable(),
  p95_ms: z.number().nullable(),
  /** Deployed but no traffic, or not running: drawn grey. */
  idle: z.boolean(),
  /**
   * Proposed (plans/feat-village-overview.md V-1): the groups whose people ran it in the range, most runs first. The
   * village walks a troop to these camps; without it, the owner's group and the groups of workflows that use it.
   */
  team_ids: z.array(z.string()).nullish(),
  /**
   * Proposed (plans/feat-village-overview.md V-10): what its failed runs cost in the range, retries included. The
   * village floats it over a broken or slow piece; absent, the piece gets no badge.
   */
  failed_spend_usd: z.number().nullish(),
})
const toolSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  brand: z.string().nullable(),
  /** The MCP server's page, when the tool is a connector. */
  connector_id: z.string().nullable(),
  /**
   * Proposed (plans/feat-village-overview.md V-12): when the tool was turned off, if it is off. The Map fades a tool
   * turned off in the last two weeks and leaves out one off for longer; absent, every tool reads as on.
   */
  disabled_at: z.string().nullish(),
})
export const SESSION_OUTCOMES = ['ok', 'failed', 'slow', 'cost'] as const
const harnessSchema = z.looseObject({
  id: z.string(),
  name: z.string(),
  /** `claude`, `codex`, `cursor`, `opencode`, … */
  harness: z.string(),
  owner_id: z.string().nullable(),
  model_ids: z.array(z.string()),
  tool_ids: z.array(z.string()),
  spend_usd: z.number().nullable(),
  /** The last sessions, oldest first. */
  sessions: z.array(z.enum(SESSION_OUTCOMES)),
})
const flowSchema = z.looseObject({
  id: z.string(),
  from: z.enum(PANELS),
  to: z.enum(PANELS),
  volume: z.number(),
  unit: z.string(),
  spend_usd: z.number().nullable(),
  /** Skips the gateway and routers (drawn dashed). */
  direct: z.boolean(),
})

const metricSchema = z.looseObject({
  label: z.string(),
  current: z.number(),
  target: z.number(),
  direction: z.enum(['higher', 'lower']),
  unit: z.string(),
  min: z.number(),
  max: z.number(),
})
const problemSchema = z.looseObject({
  id: z.string(),
  kind: z.enum(PROBLEM_KINDS),
  also: z.enum(PROBLEM_KINDS).nullable(),
  tier: z.enum(TIERS),
  title: z.string(),
  why: z.string(),
  metric: metricSchema,
  /** The last 10 days of the metric, oldest first. */
  trend: z.array(z.number()),
  /** UTC date the problem started. */
  since: z.string(),
  confidence: z.enum(['high', 'medium']),
  evidence: z.array(z.string()),
  fixed_when: z.string(),
  action: z.looseObject({ label: z.string(), target: targetSchema }),
  /** Every node it involves, the one it is about first. */
  entities: z.array(z.string()),
  flow_id: z.string().nullable(),
  /** How many runs, calls or sessions it touched (the count on its flow's badge). */
  count: z.number(),
  chain_id: z.string().nullable(),
  /** OV-2: who is on it. */
  owner_id: z.string().nullable(),
  snoozed_until: z.string().nullable(),
  handled_at: z.string().nullable(),
  /** Tier, impact, urgency and confidence in one number: the order of every list. */
  score: z.number(),
  /** It involves something the viewer owns or touches (Mine scope). */
  mine: z.boolean(),
})
const chainSchema = z.looseObject({
  id: z.string(),
  title: z.string(),
  confidence: z.enum(['high', 'medium']),
  steps: z.array(z.looseObject({ entity: z.string(), what: z.string(), detail: z.string() })),
  fix: z.string(),
  /** How many problems the fix clears. */
  clears: z.number(),
  action: z.looseObject({ label: z.string(), target: targetSchema }),
})
const opportunitySchema = z.looseObject({
  id: z.string(),
  title: z.string(),
  detail: z.string(),
  saving_usd: z.number(),
  method: z.string(),
  effort: z.string(),
  entities: z.array(z.string()),
  action: z.looseObject({ label: z.string(), target: targetSchema }),
})

export const estateSchema = z.looseObject({
  as_of: z.string(),
  /** Proposed (plans/feat-village-overview.md V-9): the workspace's plan. Absent: this version's free plan. */
  plan: z.enum(['free', 'team']).nullish(),
  /** The history window's UTC days, oldest first; the last is today. */
  days: z.array(z.string()),
  /** The viewer's person id. */
  me: z.string(),
  mine: z.array(z.string()),
  groups: z.array(groupSchema),
  people: z.array(personSchema),
  workflows: z.array(workflowSchema),
  projects: z.array(projectSchema).nullish(),
  routers: z.array(routerSchema),
  providers: z.array(providerSchema),
  models: z.array(modelSchema),
  agents: z.array(agentSchema),
  tools: z.array(toolSchema),
  harnesses: z.array(harnessSchema),
  flows: z.array(flowSchema),
  problems: z.array(problemSchema),
  chains: z.array(chainSchema),
  opportunities: z.array(opportunitySchema),
  /**
   * Proposed (plans/feat-village-overview.md V-11): the org's spend for each of `days`, oldest first, split by how it
   * is governed, and what optimizations saved. The village's meters draw it as trend lines; absent, they draw none.
   */
  daily: z
    .array(
      z.looseObject({
        /** Spend by people in a team with a budget, their coding harnesses included. */
        budgeted_usd: z.number(),
        /** Spend by people in no team with a budget. */
        unbudgeted_usd: z.number(),
        /** The budgeted teams' coding-harness spend: inside a budget, but a budget can only alert on it. */
        uncapped_usd: z.number(),
        saved_usd: z.number(),
      }),
    )
    .nullish(),
  /** The viewer's own monthly allowance, when one is set. */
  allowance: z
    .looseObject({ limit_usd: z.number(), spent_usd: z.number(), projected_usd: z.number() })
    .nullable(),
})
export const estateResponseSchema = z.looseObject({ data: estateSchema })
export const problemResponseSchema = z.looseObject({ data: problemSchema })
/** `null`: never set, the UI's defaults apply. */
export const pinsSchema = z.looseObject({ pins: z.array(z.string()).nullable() })

export type Estate = z.infer<typeof estateSchema>
export type Group = z.infer<typeof groupSchema>
export type Person = z.infer<typeof personSchema>
export type Workflow = z.infer<typeof workflowSchema>
export type EstateProject = z.infer<typeof projectSchema>
export type Router = z.infer<typeof routerSchema>
export type Provider = z.infer<typeof providerSchema>
export type Model = z.infer<typeof modelSchema>
export type EstateAgent = z.infer<typeof agentSchema>
export type Tool = z.infer<typeof toolSchema>
export type Harness = z.infer<typeof harnessSchema>
export type SessionOutcome = (typeof SESSION_OUTCOMES)[number]
export type Flow = z.infer<typeof flowSchema>
export type Metric = z.infer<typeof metricSchema>
export type Problem = z.infer<typeof problemSchema>
export type Chain = z.infer<typeof chainSchema>
export type Opportunity = z.infer<typeof opportunitySchema>

/** OV-2's body: any one change. `snoozed_until: null` and `handled: false` undo. */
export interface ProblemPatch {
  owner_id?: string | null
  snoozed_until?: string | null
  handled?: boolean
}

/** Every node the map draws, by id. */
export type NodeKind =
  'group' | 'person' | 'workflow' | 'router' | 'provider' | 'model' | 'agent' | 'tool' | 'harness'
