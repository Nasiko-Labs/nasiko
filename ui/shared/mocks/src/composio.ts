/**
 * `GET/PUT /api/settings/composio` (`oss/server/src/composio_settings.rs`): the Composio key and the toolkit picker.
 * The offered list is the server's `KNOWN_TOOLKITS`. Nothing is registered at first, the key is write-only (the
 * response says whether one is in effect and where it came from), and a PUT that switches toolkits on needs a key.
 */
import { http, HttpResponse } from 'msw'

const TOOLKITS: readonly (readonly [string, string])[] = [
  ['airtable', 'Read and write records in Airtable bases.'],
  ['asana', 'Create and manage tasks, projects, and workspaces in Asana.'],
  ['bitbucket', 'Manage repositories, pull requests, and pipelines on Bitbucket.'],
  ['clickup', 'Manage tasks, spaces, and goals in ClickUp.'],
  ['confluence', 'Create and manage pages, spaces, and content in Confluence.'],
  ['discord', 'Send messages, manage channels, and interact with Discord servers.'],
  ['dropbox', 'Upload, download, and manage files in Dropbox.'],
  ['github', 'Manage repositories, issues, pull requests, and code search on GitHub.'],
  ['gitlab', 'Manage repositories, merge requests, and CI/CD pipelines on GitLab.'],
  ['gmail', 'Read, send, and manage emails in Gmail.'],
  ['googlecalendar', 'List, create, and manage events in Google Calendar.'],
  ['googledrive', 'Upload, download, and manage files in Google Drive.'],
  ['googlesheets', 'Read, write, and manage spreadsheets in Google Sheets.'],
  ['hubspot', 'Manage contacts, deals, and marketing in HubSpot.'],
  ['intercom', 'Manage conversations, contacts, and messages in Intercom.'],
  ['jira', 'Create, update, and search Jira issues and sprints.'],
  ['linear', 'Manage issues, projects, and cycles in Linear.'],
  ['monday', 'Manage boards, items, and workflows in monday.com.'],
  ['notion', 'Create, read, and update pages and databases in Notion.'],
  ['pagerduty', 'Manage incidents, schedules, and escalation policies in PagerDuty.'],
  ['salesforce', 'Query, create, and update records in Salesforce CRM.'],
  ['sentry', 'Monitor errors, manage issues, and track releases in Sentry.'],
  ['slack', 'Send messages, read channels, and manage threads in Slack.'],
  ['stripe', 'Manage payments, customers, and subscriptions in Stripe.'],
  ['trello', 'Manage boards, lists, and cards in Trello.'],
  ['zendesk', 'Create and manage support tickets in Zendesk.'],
]

interface ComposioState {
  /** A key saved from the page (the server stores it encrypted). */
  keySaved: boolean
  enabled: Set<string>
}

const fresh = (): ComposioState => ({ keySaved: false, enabled: new Set() })
let state = fresh()

/** Drop what the mock was told (tests call this between cases). */
export const resetComposioMock = () => {
  state = fresh()
}

const display = (name: string) => name.charAt(0).toUpperCase() + name.slice(1)

function view() {
  return {
    api_key_configured: state.keySaved,
    key_source: state.keySaved ? 'ui' : 'none',
    stored_key_unreadable: false,
    toolkits: TOOLKITS.map(([toolkit, description]) => ({
      toolkit,
      display_name: display(toolkit),
      description,
      logo_url: null,
      enabled: state.enabled.has(toolkit),
    })),
    failed: [],
  }
}

export function composioHandlers(session: { loggedIn: () => boolean; superuser: () => boolean }) {
  const denied = () =>
    session.loggedIn()
      ? new HttpResponse('requires superuser', { status: 403 })
      : HttpResponse.json({ error: 'missing or invalid token' }, { status: 401 })
  return [
    http.get('/api/settings/composio', () =>
      session.loggedIn() && session.superuser() ? HttpResponse.json(view()) : denied(),
    ),
    http.put('/api/settings/composio', async ({ request }) => {
      if (!session.loggedIn() || !session.superuser()) return denied()
      const body = (await request.json().catch(() => ({}))) as {
        api_key?: string
        clear_api_key?: boolean
        toolkits?: string[]
      }
      const typed = body.api_key?.trim()
      if (typed && body.clear_api_key)
        return HttpResponse.json(
          {
            error: 'send either api_key or clear_api_key, not both',
            code: 'conflicting_key_update',
          },
          { status: 400 },
        )
      const known = new Set(TOOLKITS.map(([t]) => t))
      const wanted = body.toolkits?.map((t) => t.trim().toLowerCase())
      const unknown = wanted?.find((t) => !known.has(t))
      if (unknown)
        return HttpResponse.json(
          { error: `unknown toolkit '${unknown}'`, code: 'unknown_toolkit' },
          { status: 400 },
        )
      const keyAfter = body.clear_api_key ? false : typed ? true : state.keySaved
      if (wanted && wanted.length > 0 && !keyAfter)
        return HttpResponse.json(
          {
            error: 'add a Composio API key before switching toolkits on',
            code: 'composio_not_configured',
          },
          { status: 400 },
        )
      state.keySaved = keyAfter
      if (wanted) state.enabled = new Set(wanted)
      return HttpResponse.json(view())
    }),
  ]
}
