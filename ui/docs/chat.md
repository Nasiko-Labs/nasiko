# Chat (v1a direct chat, v1b routed chat, v1c layout)

Chat talks to your agents in the browser. **Direct chat** (v1a) talks to one agent you pick. **Routed chat** (v1b) lets **the Orchestrator** choose: it calls agents as tools and writes the reply. Specs: `plans/feat-chat.md` (v1a, state/action matrix §7.7), `plans/feat-chat-v1b.md` (v1b; end states §5.5, errors §5.6) and `plans/feat-chat-v1c.md` (v1c; §1-§9 bind). Supported server: nasiko-cloud-rs `cb3aaf0c`. Recorded harness chats are read-only.

Routes:
- `/chat`: a new chat. Choose where to send with the chip in the composer or the **Choose where to send** list: the Orchestrator or an agent (see [New chat](#new-chat-v1c));
- `/chat?agent=<uuid or name>`: a new direct chat;
- `/chat?auto=1`: a new chat with the Orchestrator (opt-in, see [Why routed is opt-in](#why-routed-is-opt-in)). The `auto` key is a URL contract and stays;
- `/chat/$sessionId`: an existing chat. An Orchestrator chat shows the Orchestrator's icon and chip and has no Stop.

Entry points: the Chat nav link, and **Try it** on a running agent's page.

## Mock quickstart (5 steps, under 2 minutes)

Needs Node 24 (`.nvmrc`; `engines` allows 22.18+).

1. `npm ci`
2. `npm run dev` (http://localhost:3000, full mock mode, logged in as admin)
3. Open http://localhost:3000/chat?auto=1
4. Type "Summarise last week's incidents" and press Enter.
5. You see "Working on your request…", then "Asking seed-support-bot…", the reply, "Answered by the Orchestrator, using seed-support-bot", and a collapsed **Activity · 1 agent**.

The seed has a direct chat with a saved reply (c001), one whose last message never got a reply (c002), an Orchestrator chat (c003), a recorded harness chat (c004) and a metadata-only one (c005).

### Mock scenarios

Every stream scenario from `common/src/mocks/chat.ts` works with `?mock=` (full mock mode only). Direct scenarios need an agent: open `/chat`, pick one, then add `&mock=<name>` (or use a chat from the rail). A test keeps this table in step with `CHAT_SCENARIOS`; an unknown name shows a dev-only banner listing the valid ones.

| Scenario | URL | Do | You see |
|---|---|---|---|
| `direct-plain` | `/chat?agent=<seed id>&mock=direct-plain` | send anything | a plain text reply |
| `direct-steps` | `/chat?agent=<seed id>&mock=direct-steps` | send anything | tool steps, then text (the direct default) |
| `terminal-usage-terminal` | `/chat?agent=<seed id>&mock=terminal-usage-terminal` | send | usage between two terminal events, saved once |
| `a2a03-message` | `/chat?agent=<seed id>&mock=a2a03-message` | send | an A2A 0.3 message reply |
| `a2a03-task` | `/chat?agent=<seed id>&mock=a2a03-task` | send | an A2A 0.3 task reply |
| `a2a10-task` | `/chat?agent=<seed id>&mock=a2a10-task` | send | an A2A 1.0 task reply |
| `lowercase-states` | `/chat?agent=<seed id>&mock=lowercase-states` | send | lowercase task states handled |
| `append-reset` | `/chat?agent=<seed id>&mock=append-reset` | send | artifact replace, then append |
| `multi-artifact` | `/chat?agent=<seed id>&mock=multi-artifact` | send | two artifacts joined |
| `hitl-options` | `/chat?agent=<seed id>&mock=hitl-options` | send, pick a region | the request card; answering resumes |
| `stream-garbage` | `/chat?agent=<seed id>&mock=stream-garbage` | send | a skipped unreadable frame |
| `failed` | `/chat?agent=<seed id>&mock=failed` | send | "The agent reported an error." |
| `empty-reply` | `/chat?agent=<seed id>&mock=empty-reply` | send | "No reply has been saved…" |
| `routed-plain` | `/chat?auto=1&mock=routed-plain` | send anything | the full routed turn (the routed default) |
| `routed-multi-agent` | `/chat?auto=1&mock=routed-multi-agent` | send | two agents, "using a and b" |
| `routed-no-tool` | `/chat?auto=1&mock=routed-no-tool` | send | "Answered by the Orchestrator", no Activity |
| `routed-empty` | `/chat?auto=1&mock=routed-empty` | send | "The Orchestrator finished without a reply." |
| `routed-sub-content-only` | `/chat?auto=1&mock=routed-sub-content-only` | send | sub-agent text isn't a reply: finished without a reply |
| `routed-two-calls-one-turn` | `/chat?auto=1&mock=routed-two-calls-one-turn` | send, open Activity | "Mixed results", "1 failed" |
| `routed-nested` | `/chat?auto=1&mock=routed-nested` | send | a nested call stays out of attribution |
| `routed-hitl` | `/chat?auto=1&mock=routed-hitl` | send, pick a region | the request card, then the resumed reply |
| `routed-hitl-truncated` | `/chat?auto=1&mock=routed-hitl-truncated` | send, answer | "Loading the saved reply…", then the saved row |
| `routed-hitl-no-reconnect` | `/chat?auto=1&mock=routed-hitl-no-reconnect` | send, answer | the server saved the reply either way |
| `routed-hitl-cancelled` | `/chat?auto=1&mock=routed-hitl-cancelled` | send, answer | the same, for a cancelled replay |
| `routed-hitl-expired` | `/chat?auto=1&mock=routed-hitl-expired&tune=RESUME_FIRST_FRAME_MS:3000` | send, answer | the reconnect stays silent; after 3 s "The reply may still be arriving." and Refresh status |
| `routed-hitl-repeat` | `/chat?auto=1&mock=routed-hitl-repeat` | send, answer | one reply however often it's replayed |
| `routed-hitl-chained` | `/chat?auto=1&mock=routed-hitl-chained` | send, answer, answer again | the second request names the same agent, then the reply |
| `routed-agent-failed` | `/chat?auto=1&mock=routed-agent-failed` | send | a reply, and Activity "1 failed" |
| `routed-policy-rejected` | `/chat?auto=1&mock=routed-policy-rejected` | send, open Activity | "Stopped by an OpenRuntime limit: fan-out." |
| `routed-failed` | `/chat?auto=1&mock=routed-failed` | send | "The Orchestrator reported an error." |
| `routed-oversized` | `/chat?auto=1&mock=routed-oversized&tune=MAX_TURN_BYTES:100000` | send | "Reply too large…", then the saved reply |
| `routed-malformed` | `/chat?auto=1&mock=routed-malformed` | send | too many unreadable frames: drains, then the saved reply |
| `routed-cut` | `/chat?auto=1&mock=routed-cut` | send | "Partial reply — completion unconfirmed" |
| `routed-400` | `/chat?auto=1&mock=routed-400` | send | "The Orchestrator couldn't read this request." |
| `routed-429` | `/chat?auto=1&mock=routed-429` | send | "Too many requests." |
| `routed-500` | `/chat?auto=1&mock=routed-500` | send | "OpenRuntime hit an error." |
| `routed-503` | `/chat?auto=1&mock=routed-503` | send | "No accessible, reachable agents…" |
| `routed-reconnect-400` | `/chat?auto=1&mock=routed-reconnect-400` | send, answer | "The reply may still be arriving." |
| `routed-reconnect-403` | `/chat?auto=1&mock=routed-reconnect-403` | send, answer | "This reply was started by another sign-in…" |
| `create-slow` | `/chat?auto=1&mock=create-slow` | send | create takes 25 s: the retry notice after 20 s |
| `direct-slow` | `/chat?agent=<id>&mock=direct-slow` | send, then open another chat | the reply streams for about a second: the rail row spins, then shows a new-reply dot |

Dev aids (dev and mock builds only; production strips them):
- `&debug=turn` shows the raw frames this tab received and, on a routed turn, its mode, operation, attempt, settle key (the trace id) and the agents asked. **Copy details** on a notice points here.
- `?tune=<key>:<ms>` (repeatable) or `localStorage['ui-lab:chat-tuning'] = '{"LOST_REPLY_AFTER_MS":5000}'` override the timings in `common/src/features/chat/tuning.ts`.

## Partial live

`VITE_NASIKO_MOCK=chat,agents,observability npm run dev:live` mocks chat with the seed agents and sessions against a live server. `chat` only works together with `agents` and `observability`; on its own the three are ignored with a warning, so seed chats never point at live agents.

## Chat v1c: layout and naming

### Naming

"Orchestrator" names the router: who chooses agents and who answers a routed chat ("Ask the Orchestrator", "Answered by the Orchestrator"). "OpenRuntime" names the platform and server: errors, limits, saving ("OpenRuntime couldn't start this chat"). Inside a sentence the router takes the article: "Waiting for the Orchestrator…", never "Ask Orchestrator".

### New chat (v1c)

- **Layout:** a hero for the target, the composer, then suggestions below it and **Pick up where you left off**. The first send opens the chat with the composer docked at the bottom.
- **Choosing a target:** the chip at the composer's bottom left ("Choose where to send ▾") opens a searchable list, labelled "Send to": the Orchestrator, then running agents, then stopped ones (selectable; Send then says why it can't send). Harness agents never appear. With no target, Enter or Send opens the list and keeps your text.
- **Default target:** the last target you chose is remembered per user (`ui-lab:chat-draft:<sub>:meta:target`, cleared on sign-out). A remembered Orchestrator is chosen at once; a remembered agent once the directory shows it running. With nothing remembered and exactly one running agent, that agent is chosen (not remembered). Nothing is chosen once you've typed.
- **Your text follows a choice** you make (picker or list row). If the new target already had a saved draft, "Replaced your saved draft · Undo" shows for 10 s; Undo swaps them back. Back/Forward, a preselect and `?agent=` resolving never move text.
- **Suggestions:** an agent shows up to 3 of its skill examples; the Orchestrator shows up to 3 examples, one per running agent, else the agent list; no target shows the Orchestrator and up to 5 running agents.

### The rail and the sidebar

The rail groups loaded chats by day (Today, Yesterday, Previous 7 days, Older, local time). Each row shows the chat's kind: an agent's monogram, the Orchestrator's route icon, a terminal icon for recorded harness chats, a crossed-out bot for a removed agent. Search (the magnifier) filters loaded rows only; Esc closes it when empty. Hovering a row shows the start of its last message. With no `sidebar_state` cookie, `/chat` opens the app sidebar as its icon rail (not stored); a cookie always wins.

### Try it (v1c M0)

| Feature | URL | Needs | Do | You see | Reset | Test |
|---|---|---|---|---|---|---|
| New chat, no target | `/chat` | nothing remembered | type, press Enter | the Send to list opens, your text stays | reset recipe | `ChatPage.v1c.test.tsx` layout and flow |
| Pick a target | `/chat` | — | open the chip, pick an agent | the hero names it, text kept, focus in the box | reset recipe | same |
| Remembered target | `/chat` | pick the Orchestrator once | open `/chat` again | the Orchestrator is chosen at once | reset recipe | preselect |
| Undo a replaced draft | `/chat?auto=1` | a saved Orchestrator draft | from an agent chat with text, pick the Orchestrator | "Replaced your saved draft · Undo" | reset recipe | drafts |
| Orchestrator examples | `/chat?auto=1` | — | click a chip | it fills the box, nothing is sent | — | test 17 |
| Kinds in the rail | `/chat/5eedc000-0000-4000-8000-00000000c004` | — | look at the rail and header | Recorded rows and chip; no composer | — | test 10 |
| Date groups, Load more | `/chat?mock=many-chats` | — | scroll, Load more | four groups; 60 rows after Load more | reload | test 11 |
| No running agents | `/chat?mock=no-agents` | — | open `/chat` | "No agent is running right now." | reload | — |
| Sidebar rail on Chat | `/chat` | no `sidebar_state` cookie | open Chat, then Agents | the icon rail on Chat, the width default elsewhere | clear the cookie | test 11 |

### Recorded harness chats (v1c M1)

A chat recorded from a coding harness (Claude Code and friends) is read-only. Each reply shows its tool calls as chips above the text: a check for succeeded, a cross for failed, denied, timed out or cancelled (the word follows the name), and a dash for "No result recorded" (pending, running) or "Status unknown". More than 5 calls collapse to "N tool calls · K failed", followed by the failed calls; open it for the first 50, then **Show all**. A chip opens Arguments, Output and Error below the chips, as plain text; a section over 2000 characters is cut, with **Copy full value**. "inferred" means the call was matched to the turn by timing; "Link unknown" means it wasn't matched at all. A session recorded under the metadata-only policy has no messages and says so.

The rail keeps recorded chats out of **Chats**: pick **Recorded (n)** above the list (it shows only when there are some; `n+` while more pages exist). The view you pick is remembered per user (`ui-lab:chat-draft:<sub>:meta:railView`), and opening a recorded chat switches to it.

| Feature | URL | Needs | Do | You see | Reset | Test |
|---|---|---|---|---|---|---|
| Tool-call chips | `/chat/5eedc000-0000-4000-8000-00000000c004` | — | open the summary, then `run_tests` | "6 tool calls · 1 failed", then Arguments and Error | — | `recorded.test.tsx` test 12 |
| Cut value | same chat, third reply | — | open `git_diff` | "Truncated. Showing the first 2000 characters." and **Copy full value** | — | same |
| Metadata-only | `/chat/5eedc000-0000-4000-8000-00000000c005` | — | open it | "Only metadata was recorded for this session…" | — | same |
| Recorded view | `/chat?mock=many-recorded` | — | pick **Recorded (50+)**, then Load more | Chats says "No live chats in the loaded list"; Recorded lists 62 | reload | same |

### Sessions → Open chat (v1c M2)

A Sessions row id is a chat id, so the Sessions row detail and the trace page header offer **Open chat** when that chat is yours. The page asks first (`GET /api/chat/sessions/{id}/messages?limit=1`): while it asks, the link's place is kept; a 404 means the session isn't one of your chats (another user's, seen as a superuser, or a deleted chat) and the link goes; any other failure says "Couldn't check chat availability" with Retry and Copy details. `weave_` ids are never asked about. Deleting a chat drops the answer, so Open chat goes too.

| Feature | URL | Needs | Do | You see | Reset | Test |
|---|---|---|---|---|---|---|
| Trace header | `/sessions/5eed-sess-pr-481` | — | look at the header | **Open chat** before Open in TokenOps | — | `openChat.test.tsx` |
| Row detail | `/sessions` on the seed's spike day | — | expand the "Review PR #481" row | **Open chat** in the detail; other rows show none | — | same |
| Probe failure | `/sessions/5eed-sess-pr-481?mock=probe-500` | — | look at the header | "Couldn't check chat availability" with Retry | reload without `mock` | same |

### Background-turn signals (v1c M3)

A reply keeps coming while you're elsewhere in the app, and the rail tells you when it lands. The row's right-hand slot shows one mark: a spinner while the reply streams, a red alert when it failed, a dot for a new reply (screen readers hear "reply in progress", "reply failed" or "new reply"). **Reply ready in <title>** sits at the bottom of the rail for the newest one: it goes when you open that chat, when you dismiss it, or after a minute of the tab being visible. On a phone the Chats button carries the dot ("Chats, 1 new reply"). While the tab is hidden its title starts with the count, e.g. "(2) Chat · OpenRuntime".

Only this tab's turns count (another tab's replies show up once you reload). The chat you have open never gets a dot unless its reply landed while the tab was hidden; it clears when you look again. A reply whose save takes longer than 20 s still counts; if that save then fails, the mark turns to the alert. Stopping a reply yourself never marks it.

| Feature | URL | Needs | Do | You see | Reset | Test |
|---|---|---|---|---|---|---|
| Spinner, then dot | `/chat/5eedc000-0000-4000-8000-00000000c001?mock=direct-slow` | — | send, then open another chat | the row spins, then a dot and **Reply ready** | reload | `ChatPage.signals.test.tsx` |
| Hidden tab | same | — | send, switch to another browser tab, wait, come back | "(1) …" in the tab title while away; clear on return | — | same |
| Debug | add `&debug=turn` | — | open the chat | "signals · seen … · reply@…" under the transcript | — | — |

### Waiting (v1c M4)

**Waiting** above the rail (always shown next to **Chats** and **Recorded**, with its count when something waits) lists the chats where an agent is waiting for you: one row per chat, the oldest request first, with the request's text and age ("2 requests" when there are several). Opening a row goes to that chat and focuses the request's card; if it was already answered or has expired, a note says so. In **Chats** such a row carries an amber count in its slot (before the spinner or dot), and on a phone the Chats button shows the count ("Chats, 2 waiting"). The list polls `GET /api/hitl/pending` every 30 s while the tab is visible (60 s, then 120 s after failures), and on focus, after you answer or dismiss a request, and when a reply pauses.

Known limits (server gaps R-1 and R-2 in `docs/designs/openruntime-chat-v1c-recommendations.md`):
- A superuser gets every user's pending requests with no owner, so only requests provably theirs show: ones this tab has seen in its own chats, or ones pointing at a chat loaded in the rail. The rest are left out, with "Requests from other people's chats aren't shown here." A request on one of your chats past the rail's loaded pages shows once that page loads.
- For a normal user, a tool (`mcp_tool`) request is only placed once its chat's history has loaded in this tab; until then it counts in "N requests couldn't be linked to a chat."
- The count covers loaded chats; it reads "n+" while more rail pages exist.

| Feature | URL | Needs | Do | You see | Reset | Test |
|---|---|---|---|---|---|---|
| Waiting list | `/chat?mock=waiting` | — | pick **Waiting (3)** | three chats, oldest first; the superuser line | reload | `waiting.test.tsx` |
| As a normal user | `/chat?mock=waiting&superuser=0` | — | pick **Waiting (3)** | "2 requests couldn't be linked to a chat." | reload | same |
| Unloaded chat | `/chat?mock=waiting,many-chats&superuser=0` | — | pick **Waiting (4+)** | "Untitled chat" first | reload | same |
| Open a request | `/chat?mock=waiting` | — | open "Plan the Q3 migration" | its request card has focus | — | same |
| Poll failing | `/chat?mock=pending-fail` | — | pick **Waiting** | "Couldn't load waiting requests" with Retry | reload | same |
| Stale | `/chat?mock=waiting,pending-flaky` | wait 30 s | look at Waiting | "Last checked …" over the last good rows | reload | same |

`?superuser=0` (mock mode only, like `?mock=`) makes the mock `me` a normal user; tests use `configureMocks({ superuser: false })`.

The one-running-agent preselect can't be set up from the URL; `ChatPage.v1c.test.tsx` covers it by serving a directory with one running agent.

### Page variants

`?mock=` takes a comma list: at most one stream scenario plus any page variants, e.g. `/chat?mock=many-chats,no-agents` (the same convention as `VITE_NASIKO_MOCK`). The dev banner names each unknown entry. Tests set them with `configureChatMock({ manyChats: true })` or `configureMocks({ variant: 'no-agents' })`. A test keeps `CHAT_PAGE_VARIANT_KEYS` (`common/src/features/chat/scenarioKeys.ts`) equal to `CHAT_PAGE_VARIANTS` (`common/src/mocks/handlers.ts`).

| Variant | Effect |
|---|---|
| `many-chats` | the opt-in 60-row rail seed (c001-c005 plus c011-c065) across every date group |
| `no-agents` | every agent reads as stopped |
| `many-recorded` | 60 recorded sessions in the last hour (c101-c160), so page 1 of the rail is all recorded |
| `probe-500` | the Open chat probe fails with a 500; chat history still loads |
| `waiting` | the opt-in pending requests: c006-c008, a `maf` step and (for a superuser) someone else's request; c060 too under `many-chats` |
| `pending-fail` | every pending poll fails with a 500 |
| `pending-flaky` | the first poll works, the next 3 fail, then it works again |

### Reset recipe (browser)

Clear the localStorage keys starting `ui-lab:chat-draft:` (drafts and the remembered target) and the `sidebar_state` cookie, then reload. The mock store resets on reload.

### A page test, end to end

```tsx
import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { expect, it } from 'vitest'
import { renderApp } from '@/test/renderApp'

it('no target: Enter opens the Send to list and keeps the text', async () => {
  renderApp('/chat')
  await screen.findByRole('region', { name: 'Choose where to send' })
  await userEvent.type(screen.getByLabelText('Message'), 'hello{Enter}')
  expect(await screen.findByRole('combobox', { name: 'Send to' })).toBeInTheDocument()
  expect(screen.getByLabelText('Message')).toHaveValue('hello')
})
```

Run one file with `npx vitest run src/features/chat/ChatPage.v1c.test.tsx`.

### Known limits

- Routed and removed-agent chats are told apart by the stored `agent_url` (no mode field on the server yet, recommendation R-3).
- The rail's search and groups cover loaded rows only.

## Stack quickstart (8 steps, under 10 minutes)

nasiko-cloud-rs is used read-only.

1. Preflight. In nasiko-cloud-rs, `just infra && just run-oss`, then `curl -fsS http://localhost:8080/health` prints `ok`. The server needs an LLM key (`OPENAI_API_KEY` in `oss/server/.env`) for titles and the orchestrator.
2. `nasiko upload oss/agents/currency-agent` (server-side build and deploy; it converts currencies at fixed rates, with no LLM and no network calls, so its answer is predictable).
3. `nasiko ps` lists `currency-agent` as running.
4. Here, `npm run dev:live`.
5. Log in as `admin` / `changeme`.
6. Open `/chat?auto=1`.
7. Send "Use the currency agent to convert 100 USD to EUR."
8. You see "Working on your request…" → "Asking currency-agent…" → a reply with 92 EUR → "Answered by the Orchestrator, using currency-agent". Clean up with `nasiko rm --name currency-agent`.

If the upload's build fails on your stack (it returned `not_found` on 2026-09-27; see the recommendations doc), use an agent that's already running and name it in the prompt, for example `simulated-agent`: "Ask the simulated-agent agent to say hello, then tell me what it replied."

### Direct chat on a stack (v1a runbook)

`nasiko upload oss/agents/simulated-agent`, then its page's **Try it** or `/chat?agent=<its uuid>`. You see "Starting chat…" for about 2 s (the LLM title), "Waiting for Simulated Agent…", lorem ipsum streaming in, then a duration chip and **View trace**. Clean up with `nasiko rm --name simulated-agent` and `npm run seed:reset`.

The live streams are shaped differently from A2A 1.0 in ways the mocks didn't cover at first: one artifact per chunk, agent frames still in their JSON-RPC envelope, and a second, server-owned task that closes after the agent's. The recorded streams are replay fixtures in `common/src/features/chat/__fixtures__/`.

## Routed smoke (X6)

`node scripts/smoke-chat-routed.ts` runs the routed flow the UI uses (create → user row → dispatch) against a live server and checks the server half: the orchestrator called the expected agent, the server saved exactly one assistant row with the turn's trace id, and the script never POSTed an assistant row. `--help` lists the options:
- `--runs 5` is the release bar (5 of 5);
- `--record` writes the scrubbed stream to `common/src/features/chat/__fixtures__/live-routed.json`;
- `--agent`/`--prompt` pick another running agent;
- auth: `NASIKO_URL` + `NASIKO_TOKEN`, then `--cluster <name>`, then the active cluster in `~/.nasiko/config.json`. It never prints the token.

The client half is `live-routed.replay.test.ts`, which replays the fixture through the real registry. It also fails loudly when the frames the UI relies on disappear (schema-drift canary). Recorded 2026-09-27 against the local stack, 5 of 5 passing with `simulated-agent`.

## Builder walkthrough: does OpenRuntime call my agent?

1. Deploy an agent with a unique name and a clear skill description.
2. Check it on its own with **Try it** (direct chat).
3. Open `/chat?auto=1` and send a prompt that needs exactly that capability.
4. Watch for "Asking <your agent>…".
5. Expand **Activity**, then your agent's row: its calls, the excerpt it returned, and **View trace**.

This shows which agents OpenRuntime observably called, not why it chose them or how good the choice was. There's no router log to show that yet.

## Who saves what

- **Direct chat:** the client saves both rows (the user message before dispatch, the reply once the stream ends); the server saves a reply that resumes after a request. If a future server adds `transcript: {user, assistant}` to session rows, the client stops writing the side marked `server`.
- **Routed chat:** the client saves the user row; the **server** saves the reply at the end of the orchestrator's stream, and only when the stream reaches the end. The client never saves a routed reply. So:
  - leaving the page is safe: in-app navigation never stops a turn;
  - closing or reloading the tab, or a hot reload of `registry.ts`/`turnRegistry.ts`, can lose the reply server-side. The composer says so while a routed reply is live, and the browser asks before the tab closes;
  - a routed reply has no Stop button, since stopping would drop it.

## Why routed is opt-in

Routed chat stays behind `/chat?auto=1` until the server can save a routed reply without the browser (recommendation 2 in `docs/designs/openruntime-chat-v1b-recommendations.md`). Making it the default also needs the router evaluation set in TODOS.md. The pieces that only exist because of that gap are marked `// scaffolding: remove when the server routed reply tap ships (rec 1)`: the drain of oversized routed streams, the composer hint, the turn-end records and the 10-minute lost-reply timing.

## What OpenRuntime enforces on a routed turn

The server runs routed turns under FlowGuard: call depth (`NASIKO_FLOW_MAX_DEPTH`, default 5), fan-out (`NASIKO_FLOW_MAX_FAN_OUT`, 20), token budget (`NASIKO_FLOW_MAX_TOKENS`, 100 000) and wall-clock time (`NASIKO_FLOW_TIMEOUT_SECS`, 120 s). Tool approvals go through requests (HITL), and each user only sees the agents they can use. The UI shows a limit hit as a stop in Activity and claims no other enforcement.

## Errors

Each routed notice has **Copy details** (phase, chat id, trace id, HTTP status and RPC code; never headers or bodies) and names its anchor below.

### errors-create
The chat couldn't be created (or the user row saved). Nothing reached OpenRuntime. **Try again** keeps your text. A slow create gives up after 20 s (`CREATE_TIMEOUT_MS`): the server titles chats with a blocking LLM call.

### errors-pending
"Couldn't load waiting requests." The first `GET /api/hitl/pending` failed, so the Waiting list has nothing to show. **Retry**; Copy details gives the HTTP status and the server's `correlation_id` for its logs. Once one poll has worked, a later failure keeps the last good rows and says when they were last checked.

### errors-probe
"Couldn't check chat availability." Sessions or the trace page asked whether this session is one of your chats, and the server answered with something other than 200 or 404. **Retry**; Copy details gives the status and the path. A 404 is not an error: the session just isn't one of your chats.

### errors-400
"The Orchestrator couldn't read this request." The server rejected the dispatch before running it (JSON-RPC -32602). **Edit and send.**

### errors-403
"The Orchestrator refused this request." Not expected on a first send at `cb3aaf0c`. Ask an admin for access.

### errors-403-reconnect
"This reply was started by another sign-in and can't be resumed here." The paused reply belongs to another user or sign-in (-32605). Copy the details and start a new chat.

### errors-reconnect
"The reply may still be arriving." Replaying a resumed reply failed (400/404/409), or sent nothing for 90 s (after the 10-minute replay buffer expires the server keeps the reconnect open with no frames for about an hour). The server may still have saved it: **Refresh status**. A resumed reply never offers Run again: its continuation already ran.

### errors-answered
"Waiting for the reply to your answer…" / "No reply was saved after your answer." You answered a routed request and no reply has been saved after it. While the server can still deliver it, the composer waits, with **Start a new chat**: about 30 minutes while delivery retries and the orchestrator's turn runs, 10 minutes when the delivery outcome is unknown, and not at all once this tab saw the resumed reply finish or the server gave up. After that the notice says no reply was saved and you can send a new message in the same chat. **Refresh status** is the only action on the notice: running the message again could repeat what the agent did with your answer.

### errors-429
"Too many requests. Limit is 30 requests a minute. Wait a minute and try again." Sends and resumes share the limit; the server sends no `Retry-After`.

### errors-500
"OpenRuntime hit an error." The request may have started: **Refresh status** before running it again. Run again asks first, since agents may already have acted.

### errors-503
"No accessible, reachable agents were available for this request." None of your agents is running or reachable. Check access and that agents are running (**Agents**).

### errors-failed
"OpenRuntime reported an error." The orchestrator ended the turn as failed; the server's text is under **Details**. Open the trace, then run it again.

### errors-cap
"Three replies are already in progress. Wait for one to finish." A tab keeps at most 3 routed replies streaming (`MAX_LIVE_TURNS`), because HTTP/1.1 allows 6 connections per origin. HTTP/2 lifts this.

### errors-policy
"Stopped by an OpenRuntime limit: <limit>." A FlowGuard limit stopped part of the turn (see above); the turn carries on. Ask your admin to raise the env var named in Activity.

### errors-smoke-preflight, -auth, -create, -user-row, -dispatch, -stream, -agent, -empty, -rows
The smoke names the stage it failed at, with the chat and trace ids:
- **preflight:** `/health` didn't answer; start the stack;
- **auth:** "Token expired or invalid; run `nasiko auth login` or `nasiko connect`.";
- **create / user-row / dispatch:** the HTTP status and body;
- **stream:** no end within 120 s; check the agent is running;
- **agent:** no `tool_call` to the expected agent; check `nasiko ps` and the prompt;
- **empty:** "orchestrator returned no text";
- **rows:** not exactly one saved reply, or the wrong trace id.

## Notes

- **Two tabs in one chat:** the server interleaves them; each tab settles only on its own reply (by trace id).
- **Ownership:** the page only sends to a chat whose history loaded, which the server checks is yours. That prevents accidental cross-writes, not a hand-made request (server issue Nasiko-Labs/nasiko-cloud-rs#492).
- **Old chats:** a chat that isn't in the rail's first pages is looked up on its own (up to 20 pages). Past that, the page says "Couldn't find this chat's details" and keeps Send off unless it knows the chat's kind.

## v1b → v1c (M0)

- URLs are unchanged: `/chat`, `?agent=`, `?auto=1` and `/chat/$sessionId` all work as before.
- Changed defaults: `/chat` no longer shows a pill chooser and a "Let OpenRuntime choose" button; it opens a new chat with a target chip and a list, and may preselect a target (above). Routed chat is named the Orchestrator. On Chat the app sidebar starts as its icon rail without a cookie.
- Storage: one new localStorage key per user, `ui-lab:chat-draft:<sub>:meta:target`, cleared with the drafts on sign-out.
- Rollback: revert the PR; nothing on the server changes.

## v1a → v1b

- `/chat` is unchanged; `?auto=1` opts into routed chat. Old `?agent=` links still go direct.
- Existing routed chats (no agent, `agent_url` null or `/api/orchestrator/a2a`) are now writable.
- A blank, oversized or non-text `?agent=` shows the banner (v1a showed the chooser).
- Supported server: `cb3aaf0c`. Rollback: revert the PR; there's no data migration.

## Troubleshooting

- **"Running in another tab"**: another tab of this browser holds the chat's lock (Web Locks). Finish or close it there.
- **"No reply has been saved for this message yet."**: the reply was lost (tab closed mid-stream, or the agent returned no text). Refresh status first; Run again asks before running the message twice, since tools may already have run.
- **"The Orchestrator finished without a reply."**: this tab saw a routed turn end with no reply text. It isn't lost: there was nothing to save.
- **"Partial reply — completion unconfirmed"** after a complete-looking reply: the server hadn't saved it after three history checks (about 10 s). Refresh status looks again.
- **"This reply may already be saved."**: the save failed without an answer. Refresh status checks history before Save again, so a retry never duplicates the row.
- **Delete is disabled**: chats with approval history can't be deleted on this server (the `hitl_requests` foreign key has no cascade).
- **Try it is missing**: it only shows for running A2A agents, never for coding harnesses.
- **Anything else on a routed turn:** reopen the chat with `&debug=turn` and copy the details.
