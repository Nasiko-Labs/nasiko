/**
 * Wire types for the MCP gateway (plans/feat-mcp.md), from nasiko-cloud-rs `2d6178e4`: the `nasiko-mcp-gateway` crate's
 * views (`connectors.rs connector_dto`, `list_connectors_view`, `get_connector_view`, `catalog.rs list_toolkits_view`,
 * `permissions.rs`) behind `oss/server/src/mcp/handlers/*`. Every route answers `{data, status_code, message}`.
 *
 * The OpenAPI spec types each `data` as `McpEnvelope` (server gap M-1), so the shapes are zod schemas here. They are
 * `z.looseObject` with only the fields the UI reads: a field the server adds never fails.
 */
import { z } from 'zod'

/** `types.rs AuthType` (the `auth_type` column). */
export const AUTH_TYPES = ['none', 'bearer', 'basic', 'oauth2', 'url_param'] as const
export type AuthType = (typeof AUTH_TYPES)[number]

/** `catalog.rs auth_flow_for`: how Connect proceeds. Toolkits send it; custom servers map `auth_type`. */
export type AuthFlow = 'none' | 'api_key' | 'oauth'

/** `permissions.rs STANCES`. `ask` pauses the agent for approval (`codes::TOOL_ASK`). */
export const STANCES = ['allow', 'ask', 'block'] as const
export type Stance = (typeof STANCES)[number]

const nullableString = z.string().nullable().optional()

/** `connectors.rs connector_dto`: what register (201) and update answer. */
export const connectorDtoSchema = z.looseObject({
  connector_id: z.string(),
  name: z.string(),
  display_name: nullableString,
  description: nullableString,
  logo_url: nullableString,
  url: nullableString,
  transport: nullableString,
  auth_type: nullableString,
  is_active: z.boolean(),
  source_kind: nullableString,
  build_status: nullableString,
  owner_id: nullableString,
  created_at: nullableString,
  updated_at: nullableString,
})
export type ConnectorDto = z.infer<typeof connectorDtoSchema>

/** The list and detail views add `is_owner`, `version`, `tool_count`, `is_connected` and `owner_username`. */
const connectorSchema = connectorDtoSchema.extend({
  is_owner: z.boolean(),
  version: nullableString,
  tool_count: z.number(),
  is_connected: z.boolean(),
  owner_username: nullableString,
})
export type Connector = z.infer<typeof connectorSchema>

/** `GET /api/mcp/connectors`: split by ownership. */
export const connectorListSchema = z.looseObject({
  created_by_you: z.array(connectorSchema),
  shared_with_you: z.array(connectorSchema),
})
export type ConnectorList = z.infer<typeof connectorListSchema>

const toolSchema = z.looseObject({ name: z.string(), description: nullableString })

/** `GET /api/mcp/connectors/{id}` (`get_connector_view`): 404 when the caller can't reach it. */
export const connectorDetailSchema = connectorSchema.extend({
  tools: z.array(toolSchema),
  is_public: z.boolean(),
  has_credential: z.boolean().optional(),
  upload_info: z
    .looseObject({
      build_status: nullableString,
      version: nullableString,
      image_tag: nullableString,
      error_msg: nullableString,
    })
    .optional(),
})
export type ConnectorDetail = z.infer<typeof connectorDetailSchema>

/** `GET /api/mcp/composio/toolkits` (`list_toolkits_view`): platform Composio connectors. */
export const toolkitListSchema = z.looseObject({
  toolkits: z.array(
    z.looseObject({
      connector_id: z.string(),
      name: z.string(),
      display_name: nullableString,
      description: nullableString,
      logo_url: nullableString,
      auth_flow: z.enum(['none', 'api_key', 'oauth']),
      tool_count: z.number(),
      is_connected: z.boolean(),
    }),
  ),
})
export type ToolkitList = z.infer<typeof toolkitListSchema>
export type Toolkit = z.infer<typeof toolkitListSchema>['toolkits'][number]

/** `POST /api/mcp/connect` (`handlers/connect.rs`): `initiated` is Composio's 201, `oauth_required` a custom server's. */
export const connectOutcomeSchema = z.looseObject({
  status: z.enum(['connected', 'initiated', 'oauth_required']),
  oauth_url: z.string().optional(),
  authorization_url: z.string().optional(),
})
export type ConnectOutcome = z.infer<typeof connectOutcomeSchema>

/** `POST /api/mcp/connectors/probe` (`probe_connector_view`). */
export const probeSchema = z.looseObject({
  auth_type: z.enum(AUTH_TYPES),
  hint: nullableString,
  supports_dcr: z.boolean().optional(),
})
export type Probe = z.infer<typeof probeSchema>

/** `credentials/status`: `{connector_id, name, connected, auth_type}`. */
export const credentialStatusSchema = z.looseObject({ connected: z.boolean() })
export type CredentialStatus = z.infer<typeof credentialStatusSchema>
/** `POST credential` (201): stored either way; `connected` says whether it verified. */
export const credentialResultSchema = z.looseObject({
  connected: z.boolean(),
  error: nullableString,
})
export type CredentialResult = z.infer<typeof credentialResultSchema>
/** `oauth/status`: `{connector_id, name, authorized, expires_at, scope}`. */
export const oauthStatusSchema = z.looseObject({
  authorized: z.boolean(),
  expires_at: nullableString,
})
export type OauthStatus = z.infer<typeof oauthStatusSchema>
export const authorizeSchema = z.looseObject({ authorization_url: z.string() })
export type Authorize = z.infer<typeof authorizeSchema>

/** `GET /api/mcp/agents/{id}/connectors` (`permissions.rs list_connectors_view`). */
export const agentConnectorsSchema = z.looseObject({
  connectors: z.array(
    z.looseObject({
      connector_id: z.string(),
      name: z.string(),
      display_name: nullableString,
      description: nullableString,
      logo_url: nullableString,
      enabled: z.boolean(),
    }),
  ),
})
export type AgentConnectors = z.infer<typeof agentConnectorsSchema>

/** `…/connectors/{cid}/tools` (`list_connector_tools_view`): each tool with this agent's effective stance. */
export const agentToolsSchema = z.looseObject({
  tools: z.array(
    z.looseObject({ name: z.string(), description: nullableString, stance: z.enum(STANCES) }),
  ),
})
export type AgentTools = z.infer<typeof agentToolsSchema>
export type AgentTool = z.infer<typeof agentToolsSchema>['tools'][number]

/** `GET /grants` (`list_shares_view`): `access_reasons` is one row per person, the most specific reason (`AccessReason`). */
export const sharesSchema = z.looseObject({
  is_public: z.boolean(),
  access_reasons: z.array(
    z.looseObject({
      user_id: z.string(),
      username: z.string(),
      display_name: nullableString,
      email: nullableString,
      role: nullableString,
      via: z.string(),
      via_label: nullableString,
    }),
  ),
})
export type Shares = z.infer<typeof sharesSchema>
export type AccessReason = z.infer<typeof sharesSchema>['access_reasons'][number]

/** `GET /consumers` (`list_consumers_view`). `teams`/`departments` are one grant type in EE (migration 1040), empty in OSS. */
export const consumersSchema = z.looseObject({
  agents: z.array(
    z.looseObject({
      agent_id: z.string(),
      agent_name: z.string(),
      agent_display_name: nullableString,
      enabled: z.boolean(),
      tools_used: z.number(),
      total_tools: z.number(),
    }),
  ),
  teams: z.array(z.looseObject({ id: z.string(), name: z.string(), created_at: nullableString })),
  departments: z.array(
    z.looseObject({ id: z.string(), name: z.string(), created_at: nullableString }),
  ),
})
export type Consumers = z.infer<typeof consumersSchema>

/** `GET /api/mcp/share-targets?q=` (`search_share_targets_view`, org-visibility scoped). */
export const shareTargetsSchema = z.looseObject({
  users: z.array(
    z.looseObject({ user_id: z.string(), username: z.string(), display_name: nullableString }),
  ),
})
export type ShareTargets = z.infer<typeof shareTargetsSchema>

/** `build-status` (`mcp/build.rs get_build_status`). */
export const buildStatusSchema = z.looseObject({
  build_status: nullableString,
  error_msg: nullableString,
  image_tag: nullableString,
})
export type BuildState = z.infer<typeof buildStatusSchema>
/**
 * `build-logs`: the container's last `tail` lines, an array (M-3: the legacy page read it as a string). Checked on the
 * whole body: an empty array is a real answer, which `apiData` would reject as "no data".
 */
export const buildLogsSchema = z.looseObject({ data: z.array(z.string()) })
export type BuildLogs = z.infer<typeof buildLogsSchema>

/** `upload` / `upload-github`: 202 `{connector_id, build_id}`. */
export const uploadResultSchema = z.looseObject({ connector_id: z.string(), build_id: z.string() })
export type UploadResult = z.infer<typeof uploadResultSchema>
