/**
 * The apps an agent can use, where a chat starts and under its reply (agentApps.ts). Connecting happens in place,
 * through the MCP catalog's own control and OAuth popup; the list refreshes when the popup closes.
 */
import { Link } from '@tanstack/react-router'
import { Check } from 'lucide-react'
import { useAgentsDirectory } from '@nasiko/features/agents/api'
import { AgentMark } from '@nasiko/features/agents/components/bits'
import { isUuid } from '@nasiko/features/agents/normalize'
import type { Skill } from '@nasiko/features/agents/types'
import { useMyConnections } from '@nasiko/features/mcp/api'
import { ConnectControl } from '@nasiko/features/mcp/components/ConnectControl'
import { authFlowOf } from '@nasiko/features/mcp/logic'
import { useOAuthPopup } from '@nasiko/features/mcp/oauth'
import type { CallerConnections } from '@nasiko/features/mcp/types'
import { type AgentApp, MAX_SHOWN_APPS, appsToConnectIn, orderApps } from '../agentApps'
import { copy } from '../copy'

type Row = CallerConnections['connectors'][number] & { auth_type?: string | null }

const toApp = (c: Row): AgentApp => ({
  id: c.connector_id,
  name: c.name,
  label: c.display_name || c.name,
  logoUrl: c.logo_url ?? null,
  authType: c.auth_type ?? null,
  status: c.status,
})

function useAgentApps(agentId: string, skills?: readonly Skill[]) {
  // Only a real agent: a routed chat's Orchestrator has no connectors of its own.
  const q = useMyConnections(agentId, isUuid(agentId))
  return orderApps(
    (q.data ?? []).map((c) => toApp(c as Row)),
    skills,
  )
}

function AppLogo({ app }: { app: AgentApp }) {
  // Toolkit logos are black single-colour icons (simple-icons): light on a dark theme.
  return (
    <span className="flex shrink-0 dark:[&_img]:invert">
      <AgentMark name={app.label} iconUrl={app.logoUrl} size={16} />
    </span>
  )
}

function AppConnect({ app, openPopup }: { app: AgentApp; openPopup: (url?: string) => void }) {
  return (
    <ConnectControl
      size="xs"
      iconOnly
      openPopup={openPopup}
      target={{
        id: app.id,
        label: app.label,
        // Composio toolkits have no auth type: connecting returns the provider's link, opened in the popup.
        authFlow: authFlowOf(app.authType),
        authType: app.authType,
        connected: app.status === 'connected',
      }}
    />
  )
}

/** Where a chat with an agent starts: the apps it works with, each connected or one click from it. */
export function AgentApps({ agentId, skills }: { agentId: string; skills?: readonly Skill[] }) {
  const apps = useAgentApps(agentId, skills)
  const openPopup = useOAuthPopup()
  if (!apps.length) return null
  const shown = apps.slice(0, MAX_SHOWN_APPS)
  return (
    <section
      className="flex flex-col items-center gap-2"
      aria-label={copy.worksWith}
      data-testid="agent-apps"
    >
      {/* The hint first: connecting is optional, the agent works on pasted text without any app. */}
      <p className="max-w-prose text-center text-xs text-balance text-muted-foreground">
        {shown.some((a) => a.status === 'needs_connection')
          ? copy.worksWithHint
          : copy.worksWithHintConnected}
      </p>
      <ul className="flex flex-wrap items-center justify-center gap-2">
        <li className="text-xs font-medium">{copy.worksWith}</li>
        {shown.map((app) => (
          <li
            key={app.id}
            className="flex items-center gap-1.5 rounded-full border py-0.5 pr-0.5 pl-2.5 text-xs"
          >
            <AppLogo app={app} />
            <span>{app.label}</span>
            {app.status === 'needs_connection' ? (
              <AppConnect app={app} openPopup={openPopup} />
            ) : (
              <Check
                className="mr-2 size-3.5 text-success"
                aria-label={copy.appConnected(app.label)}
              />
            )}
          </li>
        ))}
        {apps.length > shown.length ? (
          <li className="self-center text-xs">
            {/* The rest live on the MCP servers page, its Toolkits view: every toolkit, connected or not. */}
            <Link
              to="/mcp"
              search={{ view: 'toolkits' }}
              className="text-primary-text underline-offset-4 hover:underline pointer-coarse:inline-flex pointer-coarse:min-h-11 pointer-coarse:items-center"
              aria-label={copy.moreAppsLabel(apps.length - shown.length)}
            >
              {copy.moreApps(apps.length - shown.length)}
            </Link>
          </li>
        ) : null}
      </ul>
    </section>
  )
}

/** Under an agent's reply: Connect for each app the reply names that the viewer has not connected yet. */
export function ReplyConnect({ agentId, text }: { agentId: string; text: string }) {
  // The card's own apps, from the directory the chat page already holds (no extra read).
  const skills = (useAgentsDirectory().byId.get(agentId)?.skills ?? []) as Skill[]
  const apps = appsToConnectIn(text, useAgentApps(agentId), skills)
  const openPopup = useOAuthPopup()
  if (!apps.length) return null
  return (
    <div
      className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground"
      data-testid="reply-connect"
    >
      <span>{copy.replyConnect}</span>
      {apps.map((app) => (
        <span key={app.id} className="flex items-center gap-1.5">
          <AppLogo app={app} />
          <span className="text-foreground">{app.label}</span>
          <AppConnect app={app} openPopup={openPopup} />
        </span>
      ))}
    </div>
  )
}
