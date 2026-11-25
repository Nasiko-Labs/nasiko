# Replay evals

Replay evals turn coding tasks your team already finished with Claude Code into a private,
SWE-bench-Verified-style benchmark, replay them on other models through the opencode agent,
and report which tasks those models solve just as well and what they cost.

The output reads like: *"haiku matched the original sessions on 64% of tasks at 71% lower
cost than the original sessions, on 25 tasks (25 attempts)."*

Every model call goes through the Nasiko LLM router, so every cost is the router's own figure
(its `trace_usage` rows, priced by its price book).

The server runs the whole pipeline. The Evals section of the UI and `nasiko eval` are two
clients of the same API, `/api/evals/*`. `nasiko eval local` runs the older pipeline on your
own machine, for repositories that are not on GitHub (see the end of this page).

## How it works

| Step | What happens |
|---|---|
| Sessions | Your Claude Code sessions, synced with chat history (`nasiko integration sync`). Each carries where it ran: its folder, branch and repository (read from local git when syncing; only the host and `owner/name` leave your machine). |
| Dataset | You pick sessions. For each, the server lists the commits made during it (and up to two hours after) on its branch and the default branch, through your connected GitHub account, and links the session to the commit (or unbroken run of commits) that is its work. The commit is split into the fix (gold patch) and its tests (test patch), Rust inline `#[cfg(test)]` modules included. |
| Rewrite (P1) | A model rewrites the session's request as an issue. |
| Build and gate | The server builds an image per task (the repository at the commit before the fix, as a fresh one-commit repository, with opencode at the OpenCode harness image's version) and runs the tests several times: old code, old code + new tests, fix + new tests. This fills `FAIL_TO_PASS` and `PASS_TO_PASS` and drops flaky tests. |
| Screen (P3) | Reviewer models judge each task the way SWE-bench Verified did: is the request clear, would the tests accept any correct fix, how long would it take. A task is kept only if every score is 0 or 1 and nothing is flagged; a task no reviewer could judge waits for a person. |
| Run | You pick verified tasks and models. Each model is one of your LLM configs. A preflight call per model must come back priced. Then opencode replays every task per model in the task image, and each attempt is graded in a fresh container with no network: resolved only if every `FAIL_TO_PASS` and `PASS_TO_PASS` test passes (SWE-bench's rule). |
| Report | Resolved rate, match rate, cost, saving against the original sessions (and against the original model re-run, when you picked it), results by difficulty, and why attempts failed, each rate with a 95% range. Saved with the run. |

Building a dataset and running one take minutes to hours. Both are a durable job queue
(`eval_jobs`): any server with evals on claims the jobs, a long build or attempt keeps a
heartbeat, and a job whose server stopped is taken over.

## Before you start

* **The server runs evals:** `EVAL_ENABLED=true`, a container backend (the local Docker daemon in
  OSS), and `EVAL_ROUTER_URL`, the server's address as an eval container reaches it, without
  `/v1`: for example `http://nasiko-server:8080` with Compose, or
  `http://host.docker.internal:8082` for a server running on the Docker host.
* **The server keeps chat history:** `CODING_AGENT_CHAT_HISTORY` is on (the default in OSS).
  Tasks are built from your prompts and edits, which are not kept without it.
* **GitHub is connected:** your repositories are read through your GitHub account
  (`nasiko github login`, or Connect GitHub in the Evals section).
* **Every model you test is one of your LLM configs** (Settings → LLM router, or `/api/llm-configs`),
  with a price in the router's price book: a model the router cannot price stops at the
  preflight rather than producing a guessed cost. The attempt runs on the config's own model,
  pinned, so neither tier routing nor a fallback stands in for it.
* Only Claude Code sessions and Cargo (Rust) repositories, so far.

### A local model served by llama-server

1. Start it, for example `llama-server -m flint.gguf --port 8092 --jinja -c 32768`.
2. As a superuser, register it as a custom provider:
   `POST /api/custom-providers {"display_name":"llamacpp","base_url":"http://host.docker.internal:8092/v1","kind":"openai","api_key":"local"}`.
3. Give it a price of zero (there is no admin API for prices yet; this is a one-time SQL insert):

   ```sql
   INSERT INTO model_pricing (provider, model, input_price_per_1m, output_price_per_1m,
     cache_read_price_per_1m, cache_creation_price_per_1m, notes)
   VALUES ('llamacpp', 'flint', 0, 0, 0, 0, 'operator: local model, free');
   ```

4. Create an LLM config on that provider and model.

## In the UI

Evals is a three-step stepper over Claude Code's sessions: tick sessions (the ones that can't
become a task say why) → Generate evals dataset → wait while it is built → tick tasks and
models (the sessions' own model is ticked: the fair comparison) → Run evals → the task × model
table, with the original session as a column. The dataset and run are in the URL, and saved
runs are listed under the sessions.

## With the CLI

```sh
nasiko eval harnesses
nasiko eval sessions --harness claude --limit 20
nasiko eval datasets create --limit 20 --wait            # newest usable sessions
nasiko eval datasets create --session <id> --session <id> --reviewer opus --wait
nasiko eval tasks <dataset>
nasiko eval review <task> --clarity 0 --validity 1 --effort "15 min - 1 hour"
nasiko eval runs create --dataset <dataset> --model haiku --model opus --wait
nasiko eval runs ls | show <run> | cancel <run> | rm <run>
nasiko eval report <run> [--format text|json|md]
```

Models, rewriters and reviewers are named by LLM config name (or id). The rewriter and
reviewers default to the config of the model the sessions mostly used: a model that rewrote or
screened the tasks still runs, and its scores are marked optimistic.

## API

| Route | What it does |
|---|---|
| `GET /api/evals/harnesses` | Your connected harnesses, and whether GitHub is connected, chat history is kept and the server runs evals |
| `GET /api/evals/harnesses/{id}/sessions` | Its newest sessions, each eligible or with the reason it is not |
| `POST /api/evals/datasets` `{harness, session_ids, rewriter_id?, reviewer_ids?, gate_runs?}` | `202`, the dataset, harvesting |
| `GET /api/evals/datasets[/{id}]` | Your datasets; one with its tasks, the outcome per session and the models you can test |
| `GET /api/evals/tasks/{id}`, `POST /api/evals/tasks/{id}/review` | A task in full; a person's verdict |
| `GET/PUT /api/evals/specs/{owner}/{repo}` | The repository's build and test settings, proposed on first use |
| `POST /api/evals/datasets/{id}/runs` `{task_ids, model_ids, attempts?}` | `202`, the run, queued |
| `GET /api/evals/runs[/{id}]` | Your runs; one with a result per attempt (score, resolved, cost, time, failure) and its report |
| `GET /api/evals/runs/{id}/attempts/{attempt_id}` | The graded patch, the grade and the logs |
| `POST /api/evals/runs/{id}/cancel`, `DELETE …` | Cancel a run; delete a run or a dataset |
| `GET /api/evals/usage?run=`, `POST /api/evals/price` | The router's totals per attempt; a price for token counts it never saw |

Every object is its owner's: another user's id reads as `404`.

## What runs where

* **Agent container**: the task image on a network that reaches the model router's `/v1/*` and
  nothing else (OSS: an `--internal` Docker network and an nginx gateway; the gateway refuses
  every other path). opencode authenticates with a token minted for that attempt alone: it names
  the attempt the router files the calls under and the LLM config they run on, so the agent
  cannot tag its calls as another attempt or pick another model.
* **Grading and gate containers**: the same task image with no network: apply the model's patch
  (test files and its edits to inline Rust tests removed), apply the hidden test patch, build and
  run the tests.
* Every container runs with all Linux capabilities dropped, `no-new-privileges`, CPU, memory,
  process and disk limits (agent 2 GiB, grader 8 GiB) and a wall-clock limit (45 minutes per
  attempt). When an attempt is stopped for a limit, its changes are saved first and graded.

## On your own machine: `nasiko eval local`

For repositories that are not on GitHub, `nasiko eval local …` runs the same pipeline on your
machine: local Claude Code logs, local git and Docker, files under `~/.nasiko/eval/`
(`NASIKO_EVAL_HOME`). `nasiko eval local init` creates `config.json` (models by router provider
and model, rewriter, reviewers, control, limits); then `harvest --repo`, `rewrite`,
`spec detect`, `build`, `screen`, `review`, `freeze --name`, `run --dataset --model`,
`report`, `costs`, `export`, `clean`. Its model calls still go through the router, as your
`eval` identity, one model at a time.

## Limits and known gaps

* Claude Code sessions and Cargo repositories only.
* A session is usable only if it was synced with chat history, and after its repository was
  recorded (re-sync older sessions to backfill it).
* Tests run per affected Cargo package; integration tests that need a database or network fail
  in the sandbox and drop out of the lists.
* Few sessions end in a single clean commit, so a meaningful score needs many sessions.
* On Kubernetes, the batch runner is not built yet: evals need the Docker runtime for now.
* Attempt tags live with the router's hot usage rows (90 days).
