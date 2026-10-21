# OSS Contribution Copilot

An A2A agent that ships open-source pull requests while keeping a human in charge of every step that matters.

## What it does

You give it a repository (`contribute to Nasiko-Labs/nasiko`). It reads the repo, picks one small contribution that it can make safely, prepares the exact files, and opens a pull request from your fork. It stops at three human approval gates along the way:

| Gate | Question | What you review |
|---|---|---|
| **G1** | Approve this contribution idea? | Title and reason |
| **G2** | Approve these exact files for commit? | Every file's full contents (on the detail page) |
| **G3** | Publish: push a branch and open the PR? | The final PR title and body |

The agent cannot pass a gate on its own. A decision only counts for the gate it names, and a `reject` stops the run.

## Safety controls

- **Deny-by-default allow-list.** It writes only to repos listed in `WRITABLE_REPOS` (one per entry, no wildcards), and to your own fork of those repos. It checks that the fork's parent is on the list before pushing.
- **New files only.** It can add files under `agents/` or `docs/`. It never overwrites an existing file and rejects `..` or absolute paths.
- **Fork-only writes.** It never pushes to the upstream repo and never force-pushes. Every change goes up as one commit on a new `copilot/<thread>` branch.
- **Cost budget.** Every LLM call is metered, with a per-thread cap (`MAX_COST_USD`). The PR body states the total cost.
- **Approver key.** If `APPROVER_KEY` is set, gate decisions need the `X-Approver-Key` header. Approving over A2A is off unless you set `ALLOW_A2A_APPROVALS=1`.
- **Audit log.** Every step and decision is timestamped on `/work/<thread_id>`.

## Contribution modes

| Mode | What it contributes |
|---|---|
| `self` | This agent's own directory, added to `agents/oss-contribution-copilot` (Nasiko repo only). Needs no LLM. |
| `docs` | A README for an example agent that has none. It reads that agent's source files and writes the README with an LLM. |
| `auto` (default) | `self` when that is possible, otherwise `docs`. |

## Run it

```bash
cp .env.example .env    # set GITHUB_TOKEN (public_repo scope); OPENAI_API_KEY for docs mode
docker compose up --build
curl localhost:8001/health
```

On Nasiko: `nasiko deploy .` then `nasiko chat "contribute to Nasiko-Labs/nasiko"`.

## Interfaces

**A2A (JSON-RPC on `/`, card at `/.well-known/agent-card.json`).** Accepts plain-text commands: `contribute to owner/repo`, `status t_…`, `approvals`.

**REST (for chat front-ends like DronaHQ).** Every response is a projection of under 200 tokens: `thread_id, repo, phase, headline, gate, next, cost_usd, pr_url, detail_url`.

| Method | Path | Body / query |
|---|---|---|
| POST | `/a2a/contribute` | `{"repo": "owner/name"}` |
| GET | `/a2a/status` | `?thread_id=t_…` |
| GET | `/a2a/approvals` | — |
| POST | `/a2a/approve` | `{"gate_id": "G1", "decision": "approve", "note": "", "thread_id": "t_…"}` |
| GET | `/work/{thread_id}` | Human-readable detail page |

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `GITHUB_TOKEN` | For G3 | Classic PAT with `public_repo` scope |
| `OPENAI_API_KEY` | Docs mode | OpenAI-compatible key (`OPENAI_BASE_URL` optional) |
| `MODEL` | No | Default `gpt-4o-mini` |
| `WRITABLE_REPOS` | No | Default `Nasiko-Labs/nasiko` |
| `PUBLIC_URL` | No | Base URL used in `detail_url` |
| `APPROVER_KEY` | No | Required header value for gate decisions |
| `MAX_COST_USD` | No | Per-contribution LLM budget, default `0.50` |
| `CONTRIBUTION_MODE` | No | `auto`, `self` or `docs` |
