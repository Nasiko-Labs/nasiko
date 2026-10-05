# OpenRuntime UI Lab

A React experiment for Nasiko's OpenRuntime frontend. It tests the stack from DOC-2026-031:
React 19 + Vite (static SPA), TanStack Router/Query, Tailwind v4 + shadcn/ui, Recharts, Motion.

What's here:
- **Overview** (`/`): the homepage. Needs you (waiting requests, agents that need action, budget warnings, failing
  sessions), fleet Spend, Fleet health (each agent rated Healthy / Watch / Needs action / Unknown with the reason; each
  count opens `/agents?health=<rating>`), coding harnesses, recent sessions and quick actions. A source that can't be
  checked says so with Retry, so "Nothing needs you" only shows when every source answered. The backend status page
  is at `/status`.
- **The observability demo**: follow the money from a spend spike to the span that caused it.
  - **TokenOps** (`/tokenops`): a plain-English summary first, details in disclosures.
  - **Sessions** (`/sessions`): the fleet pulse, or one day's sessions ranked by cost.
  - **Session trace** (`/sessions/:id`): a narrated waterfall with the failing span open.
  - Demo walkthrough: `docs/designs/openruntime-demo-script.md`.
- **TokenOps** detail (`/tokenops`): LLM cost organised around five operator questions:
  - Where am I this month?
  - Why did spend spike?
  - Who drives cost?
  - Which traces burned it?
  - Is cost buying performance?
- **Harnesses** (`/harnesses`): coding-harness usage (Claude Code, Codex, OpenCode, Cursor) from Org down to
  one developer. Needs a proposed endpoint; without it the page shows your own usage from existing ones.
- **Agents** (`/agents`): the catalog of every agent you can see, Your agents (`/agents/mine`) with the ones that
  need attention pinned first, and one agent's page (`/agents/:id`): health, activity, versions, access and settings.
  Restart, Start and roll back report what actually happened, not just that the call returned.
- **Deploy and Builds** (`/deploy`, `/builds`, `/builds/:id`): deploy an agent from the browser by uploading a zip
  (checked in the browser the way the server checks it), from a GitHub repository, or from a registry image. The Builds
  list pins in-progress builds on top, a Build page follows one build live and ends with Chat with it or the fix, each
  agent page has a Builds tab, and a build you leave keeps being followed (a sidebar count and one toast when it
  finishes). **Deploy an agent** buttons replace the CLI-only empty states; the CLI steps stay underneath.
- **Chat** (`/chat`, `/chat/:id`): talk to one agent in the browser, with streamed replies, tool steps, answerable
  requests and the trace one click away, or let OpenRuntime choose the agents (`/chat?auto=1`, routed chat).
  **Try it** on a running agent's page opens a chat with it. Guide: `docs/chat.md`.
- **Weave fixtures** (`/weave`, EE dev builds only): a React renderer for Weave's generated dashboards.
- **App shell**: a sidebar grouped by goal (collapses to an icon rail with ⌘B / Ctrl+B, a sheet on phones), a
  theme menu (System / Light / Dark, and the Teal, Indigo, Plum or Mist/Carbon theme) and Sign out, synced across tabs. Design reference:
  `DESIGN.md`.

## Quickstart: mock data (about 2 minutes)

Needs Node 24 (`.nvmrc`; `engines` allows 22.18+, which `scripts/*.ts` need to run without a build step).

```sh
npm install
npm run dev            # http://localhost:3000/tokenops  — "MOCK DATA" badge in the sidebar footer
```

Every API call is answered in the browser from a deterministic seed: 60 days of data, 20 agents, a spike day,
unpriced calls and a deleted agent. No server needed.
The agent pages mutate that seed in memory, so Restart, Stop, roll back, secrets and access changes round-trip until
you reload. One seed agent is crashed, one failed, one deploying and one stopped, so every status shows.

For the demo, pin the date so the figures match the script:
`http://localhost:3000/tokenops?anchor=2026-09-26&demo=1`. Degraded states for QA:
`?mock=tempo-down|empty|trace-503|trace-500|scan-fail`; for the app shell, `?mock=server-down` (a stopped server) and
`?mock=logout-unavailable` (a failed sign-out).

**Deploy in mock mode.** Uploads, GitHub clones and registry imports all complete against the seed, and a new agent
reads `deploying` until its build settles. States: `?mock=deploy-build-fails` (the upload is accepted, the build fails),
`deploy-github-unconfigured|deploy-github-disconnected|deploy-github-no-repos`,
`deploy-registry-disabled|deploy-registry-not-running`, `deploy-no-rights` (EE without deploy rights) and
`builds-absent` (a server without `/api/builds`). Navigate in-app while a build runs: a full reload resets the mock.

**Harnesses in mock mode.** A separate seed (60 developers, 11 units: 3 root units, 7 teams and one nested unit, 4 harnesses plus an unknown one)
answers the page. The OSS build (`npm run dev`) shows the viewer's own usage; `?as=<seed username>` picks the viewer
(default `admin`, the superuser; `?as=sam` is a non-superuser, whose viewer comes from the `/api/me` claims).
The EE build (`npm run dev -- ee`) adds the org levels and a "View as" menu:
- `/harnesses?as=admin` (default): Org level.
- `?as=maya`: manager of one unit (lands on Engineering). `?as=omar`: two units (Your units). `?as=lena`: nested units.
- `?as=tom` (manager who leads nothing) and `?as=sam` (member): their own Individual view. `?as=root`: superuser.

States: `?mock=usage-404|prev-fail|usage-500|all-unpriced|no-activity`, plus `drill-404` in the EE build.

## Quickstart: live OSS server

```sh
# in nasiko-cloud-rs (unchanged by this repo)
just run-stack

# here
npm run seed:live      # optional: load seed rows into the local Postgres (npm run seed:reset removes them)
npm run dev:live       # log in as admin / changeme
```

**Chat.** `/chat` talks to one agent in the browser, and `/chat?auto=1` lets OpenRuntime choose. `docs/chat.md` has a mock quickstart (5 steps), a stack quickstart (8 steps), every mock scenario, the routed smoke and the error guide. Server gaps: `docs/designs/openruntime-chat-recommendations.md` (v1a) and `docs/designs/openruntime-chat-v1b-recommendations.md` (v1b).

**Live data for Sessions and Session trace.** These read chat sessions and Tempo traces, which
`seed:live` doesn't create:
1. Start the stack with `TEMPO_URL` and `LOKI_URL` set on nasiko-server.
2. Deploy an agent and talk to it with `nasiko chat <agent>`.
3. Reload `/sessions`.

Without trace data the page still lists the sessions (each opens its own page) under a note that
says why cost and tokens are missing and how to fix it. On windows of 7 days or more (7d, 30d,
Last month, This month after the 7th) the server's Tempo search is longer than Tempo allows, so every row lacks trace data even
with Tempo running; pick 24h or open a session. This is best-effort, not a ship gate.

**Live data for Harnesses.** `seed:live` also inserts coding-agent rows, their `trace_usage` turns, chat sessions
and messages (`5eed0002…` ids). These are real rows in the shared tables, so live TokenOps and Sessions numbers
include the harness traffic too; a seed agent the CLI later adopts (`nasiko agents install`) is kept on reset. The org endpoint doesn't exist yet, so live mode shows the "not available on this server"
line and your own usage. `VITE_NASIKO_MOCK=harnesses npm run dev:live` previews the page with mock data (in the EE
build, `npm run dev:live:ee`, the org levels too). The proposed contract: `docs/designs/openruntime-harness-recommendations.md`.

**Live contract.** `npm run record:live` records seed-only server responses into `common/src/test/__live__/` (it starts its own
throwaway server and database, so the dev stack is untouched), and `npm test` checks the mocks, types and error handling
against them. EE runs on its own `nasiko_ee` database: `npm run ee:server`, `npm run seed:ee`, `npm run record:live --
--edition ee`, and `npm run dev:live:ee` for the lab against it. Guide: `docs/live-contract.md`.

Trace drill-down relies on a proposed `/finops/top-traces` endpoint that the server doesn't have yet. To preview
it against live data, run `VITE_NASIKO_MOCK=top-traces npm run dev:live`. Deploy works against the live server as it
is; `VITE_NASIKO_MOCK=deploy,agents npm run dev:live` previews the deploy pages on mock data (the two keys go together). All env vars are listed in `.env.example`.

## Check

```sh
npm test               # typecheck + ESLint (zero warnings) + vitest
npm run lint           # ESLint + Prettier check (npm run format writes)
npm run build          # production build; then npm run budgets checks its size
npm run e2e            # the demo flows in Chromium, with axe on every page (mock mode, port 3917)
npm run test:stories   # every Storybook story as a browser test with axe (npm run storybook for the UI)
npm run knip           # unused files, dependencies and exports
```

The improvement plan these checks come from (and what each phase changed) is `docs/lab-vs-react-migration-review.md`.

Workflow: gstack `/autoplan` → build → `/review` → `/qa` → `/ship` → `/land-and-deploy`.
Conventions for humans and agents are in `CLAUDE.md`; the current plan is in `plans/feat-observability-demo.md`
(TokenOps: `plans/feat-tokenops-page.md`; Harnesses: `plans/feat-harness-org-view.md`; Agents: `plans/feat-agents.md`; Chat: `plans/feat-chat.md`, routed chat: `plans/feat-chat-v1b.md`; app shell: `plans/feat-app-shell.md`; Overview: `plans/feat-overview.md`; Deploy and Builds: `plans/feat-deploy.md`). Which legacy pages are rebuilt and which are next: `docs/rebuild-status.md`. Server gaps found along the way: `docs/designs/openruntime-server-recommendations.md`
(agent pages: `docs/designs/openruntime-agents-recommendations.md`; app shell: `docs/designs/openruntime-app-shell-recommendations.md`; Overview: `docs/designs/openruntime-overview-recommendations.md`; Deploy and Builds: `docs/designs/openruntime-deploy-recommendations.md`).
Design: `docs/designs/openruntime-observability-demo.md`; what the stack cost: `docs/designs/openruntime-stack-scorecard.md`.
Release notes are in `CHANGELOG.md` and deferred work is in `TODOS.md`.
