# Open Runtime UI

The browser app for Nasiko's Open Runtime: React 19 + Vite (static SPA), TanStack Router/Query,
Tailwind v4 + shadcn/ui, Recharts, Motion. The server embeds the built bundle and serves it same-origin.

What's here:
- **Home** (`/`): the scoped home (Yours and Fleet). Needs you (waiting requests, agents that need action, failing
  sessions), a totals strip (spend, saved tokens, agents needing attention) and one row per agent or coding harness
  with its health (Healthy / Watch / Needs action / Unknown, linking to `/agents?health=<rating>`), spend, budget use
  and last change, each opening the agent's hub. A source that can't be checked says so with Retry, so Needs you only
  says nothing needs you when every source answered. The backend status page is at `/status`.
- **The observability demo**: follow the money from a spend spike to the span that caused it.
  - **TokenOps** (`/tokenops`): a plain-English summary first, details in disclosures.
  - **Sessions** (`/sessions`): the fleet pulse, or one day's sessions ranked by cost.
  - **Session trace** (`/sessions/:id`): a narrated waterfall with the failing span open.
- **TokenOps** detail (`/tokenops`): LLM cost organised around five operator questions:
  - Where am I this month?
  - Why did spend spike?
  - Who drives cost?
  - Which traces burned it?
  - Is cost buying performance?
- **Optimization** (`/optimization`): what context optimization saved your chats, with a trend you can pick a day or
  week from, the agents with Token optimization off (one confirmed turn-on for all of them), a By agent table, the
  biggest senders, what's optimizing your tokens (each mechanism, where it is set, how many agents have it on) and your
  own history settings under "Your settings". Superusers set tier sizes under Settings → Workspace → Optimization tiers.
- **Harnesses** (`/harnesses`): your coding-harness usage (Claude Code, Codex, OpenCode, Cursor).
- **Agents** (`/agents`): the catalog of every agent you can see, Your agents (`/agents/mine`) with the ones that
  need attention pinned first, and one agent's page (`/agents/:id`): health, activity, versions, access and settings.
  Restart, Start and roll back report what actually happened, not just that the call returned.
- **Deploy and Builds** (`/deploy`, `/builds`, `/builds/:id`): deploy an agent from the browser by uploading a zip
  (checked in the browser the way the server checks it), from a GitHub repository, or from a registry image. The Builds
  list pins in-progress builds on top, a Build page follows one build live and ends with Chat with it or the fix, each
  agent page has a Builds tab, and a build you leave keeps being followed (a sidebar count and one toast when it
  finishes). **Deploy an agent** buttons replace the CLI-only empty states; the CLI steps stay underneath.
- **Chat** (`/chat`, `/chat/:id`): talk to one agent in the browser, with streamed replies, tool steps, answerable
  requests and the trace one click away, or let Open Runtime choose the agents (`/chat?auto=1`, routed chat).
  **Try it** on a running agent's page opens a chat with it. Guide: [docs/chat.md](docs/chat.md).
- **App shell**: a sidebar grouped by goal (collapses to an icon rail with ⌘B / Ctrl+B, a sheet on phones), a
  theme menu (System / Light / Dark, and the Teal, Indigo, Plum or Mist/Carbon theme) and Sign out, synced across tabs.
  Design reference: [DESIGN.md](DESIGN.md).

## Quickstart: mock data (about 2 minutes)

Needs Node 24 (`.nvmrc`; `engines` allows 22.18+, which `scripts/*.ts` need to run without a build step).

```sh
corepack enable        # once: pnpm, at the version package.json names (corepack ships with Node 24)
pnpm install
pnpm run dev           # http://localhost:3000/tokenops  — "MOCK DATA" badge in the sidebar footer
```

Every API call is answered in the browser from a deterministic seed: 60 days of data, 20 agents, a spike day,
unpriced calls and a deleted agent. No server needed.
The agent pages mutate that seed in memory, so Restart, Stop, roll back, secrets and access changes round-trip until
you reload. One seed agent is crashed, one failed, one deploying and one stopped, so every status shows.

For the demo, pin the date so the figures match: `http://localhost:3000/tokenops?anchor=2026-09-26&demo=1`.
Degraded states for QA: `?mock=tempo-down|empty|trace-503|trace-500|scan-fail`; for the app shell,
`?mock=server-down` (a stopped server) and `?mock=logout-unavailable` (a failed sign-out).

**Deploy in mock mode.** Uploads, GitHub clones and registry imports all complete against the seed, and a new agent
reads `deploying` until its build settles. States: `?mock=deploy-build-fails` (the upload is accepted, the build fails),
`deploy-github-unconfigured|deploy-github-disconnected|deploy-github-no-repos`,
`deploy-registry-disabled|deploy-registry-not-running` and `builds-absent` (a server without `/api/builds`).
Navigate in-app while a build runs: a full reload resets the mock.

**Harnesses in mock mode.** A separate seed (60 developers, 4 harnesses plus an unknown one) answers the page, showing
the viewer's own usage. `?as=<seed username>` picks the viewer (default `admin`, the superuser; `?as=sam` is a
non-superuser, whose viewer comes from the `/api/me` claims). States: `?mock=usage-404|prev-fail|usage-500|all-unpriced|no-activity`.

## Quickstart: live server

```sh
# from the repository root
just run-stack

# here
pnpm run seed:live     # optional: load seed rows into the local Postgres (pnpm run seed:reset removes them)
pnpm run dev:live      # log in as admin / changeme
```

**Chat.** `/chat` talks to one agent in the browser, and `/chat?auto=1` lets Open Runtime choose.
[docs/chat.md](docs/chat.md) has a mock quickstart (5 steps), a stack quickstart (8 steps), every mock scenario, the
routed smoke and the error guide.

**Live data for Sessions and Session trace.** These read chat sessions and Tempo traces, which
`seed:live` doesn't create:
1. Start the stack with `TEMPO_URL` and `LOKI_URL` set on nasiko-server.
2. Deploy an agent and talk to it with `nasiko chat <agent>`.
3. Reload `/sessions`.

Without trace data the page still lists the sessions (each opens its own page) under a note that
says why cost and tokens are missing and how to fix it. On windows of 7 days or more (7d, 30d,
Last month, This month after the 7th) the server's Tempo search is longer than Tempo allows, so every row lacks trace
data even with Tempo running; pick 24h or open a session. This is best-effort, not a ship gate.

**Live data for Harnesses.** `seed:live` also inserts coding-agent rows, their `trace_usage` turns, chat sessions
and messages (`5eed0002…` ids). These are real rows in the shared tables, so live TokenOps and Sessions numbers
include the harness traffic too; a seed agent the CLI later adopts (`nasiko connect <agent>`) is kept on reset.
`VITE_NASIKO_MOCK=harnesses pnpm run dev:live` previews the page with mock data.

**Live contract.** `pnpm run record:live` records seed-only server responses into `shared/testing/src/__live__/` (it
starts its own throwaway server and database, so the dev stack is untouched), and `pnpm test` checks the mocks, types
and error handling against them.

Trace drill-down relies on a proposed `/finops/top-traces` endpoint that the server doesn't have yet. To preview
it against live data, run `VITE_NASIKO_MOCK=top-traces pnpm run dev:live`. Deploy works against the live server as it
is; `VITE_NASIKO_MOCK=deploy,agents pnpm run dev:live` previews the deploy pages on mock data (the two keys go
together). All env vars are listed in `.env.example`.

## Check

```sh
pnpm test              # typecheck + ESLint (zero warnings) + vitest
pnpm run lint          # ESLint + Prettier check (pnpm run format writes)
pnpm run build         # production build; then pnpm run budgets checks its size
pnpm run e2e           # the demo flows in Chromium, with axe on every page (mock mode, port 3917)
pnpm run knip          # unused files, dependencies and exports
```
