# Replay evals (`nasiko eval`)

Replay evals turn coding tasks your team already finished with Claude Code into a private,
SWE-bench-Verified-style benchmark, replay them with other models through the opencode agent,
and report which tasks those models solve just as well and what they cost.

The output reads like: *"flint matched the original sessions on 64% of tasks at 100% lower cost
than the original sessions, on my-repo@1 (25 attempts)."*

Every model call goes through the Nasiko LLM router, so every cost in a report is the router's
own figure (its `trace_usage` rows, priced by its price book).

## How it works

| Step | Command | What happens |
|---|---|---|
| 1 | `harvest` | Reads Claude Code logs (`~/.claude/projects/*/*.jsonl`), links each session to the commit it produced, and splits that commit into the fix (gold patch) and its tests (test patch). Rust inline `#[cfg(test)]` modules are split out too. |
| 2 | `rewrite` | Rewrites the session's request as an issue (prompt P1), through the router. |
| 3 | `spec detect`, `build` | Builds a Docker image per task (the repo at the commit before the fix, as a fresh one-commit repository, with opencode installed at the same version as the OpenCode harness image) and runs the tests several times: old code, old code + new tests, fix + new tests. This fills `FAIL_TO_PASS` and `PASS_TO_PASS` and drops flaky tests. |
| 4 | `screen` | Reviewer models judge each task the way SWE-bench Verified did (prompt P3): is the request clear, would the tests accept any correct fix, how long would it take. The worst score counts; a task is kept only if every score is 0 or 1 and nothing is flagged. |
| 4b | `review` | A person decides only the tasks no reviewer model could judge (for example when no reviewers are configured). |
| 5 | `freeze` | Writes every verified task into a new, unchangeable dataset version (SWE-bench `.jsonl`). |
| 6 | `run` | Runs opencode on every task with each model, in Docker, then grades each attempt in a fresh container with no network: resolved only if every `FAIL_TO_PASS` and every `PASS_TO_PASS` test passes (SWE-bench's rule). |
| 7 | `report` | Resolved rate, match rate, cost, saving, results by difficulty, and why attempts failed, each rate with a 95% range. |

## Before you start

* Docker (or Podman, via `NASIKO_CONTAINER_CLI`).
* A Nasiko cluster you are logged in to (`nasiko auth login`), on a version that has replay
  evals: the `eval` coding-agent identity, the `x-nasiko-eval-attempt` tag and `/api/evals/*`.
* Each model you want to compare reachable by the router: a built-in provider, or a custom
  provider (`/api/custom-providers`) for anything OpenAI-compatible, such as a local llama-server.
* A price for each model in the router's price book. A model the router cannot price stops a run
  at its preflight check rather than producing a guessed cost.

### A local model served by llama-server

1. Start it, for example `llama-server -m flint.gguf --port 8092 --jinja -c 32768`.
2. As a superuser, register it as a custom provider:
   `POST /api/custom-providers {"display_name":"llamacpp","base_url":"http://localhost:8092/v1","kind":"openai","api_key":"local"}`.
   Use `http://host.docker.internal:8092/v1` instead when the server itself runs in Docker.
3. Give it a price of zero (there is no admin API for prices yet; this is a one-time SQL insert):

   ```sql
   INSERT INTO model_pricing (provider, model, input_price_per_1m, output_price_per_1m,
     cache_read_price_per_1m, cache_creation_price_per_1m, notes)
   VALUES ('llamacpp', 'flint', 0, 0, 0, 0, 'operator: local model, free');
   ```

   The provider must equal the custom provider's label. The price sync never overwrites a row
   whose note does not start with `portkey`, `openrouter`, `seed:` or `boot seed`.

## Configuration

`nasiko eval init` creates `~/.nasiko/eval/config.json` (move the folder with
`NASIKO_EVAL_HOME`):

```json
{
  "models": {
    "flint":  { "provider": "llamacpp",  "model": "flint" },
    "sonnet": { "provider": "anthropic", "model": "claude-sonnet-5-5" }
  },
  "rewriter": "sonnet",
  "reviewers": ["sonnet"],
  "control": null,
  "workers": 4,
  "storage_mib": 2048,
  "context_tokens": 32768,
  "output_tokens": 8192,
  "max_steps": 150,
  "attempt_minutes": 45
}
```

* `models`: short name → router provider label and model id.
* `rewriter`, `reviewers`: models for prompts P1 and P3. Use stronger models than the ones you
  test: `run` refuses to test a model that rewrote or screened the dataset, unless you pass
  `--allow-builder` to run it as a reference (the report marks its scores as optimistic). With no reviewers,
  every task goes to `nasiko eval review`.
* `control`: a model to re-run in the same harness as the fair comparison. Without one, models
  are compared with the original sessions (whose real fixes solved every task) and their cost.
* `storage_mib`: disk the agent's container may write before it is stopped (2 GiB by default).
  When it is stopped for a limit, its changes are saved first and graded as usual.
* `grade_storage_mib`: disk for the grading and gate containers (8 GiB by default): they build
  the crate's test binaries, and a correct fix must not fail because the grader ran out of room.
* `max_steps`, `attempt_minutes`: the agent's step and time budget per attempt.

## Commands

```sh
nasiko eval init
nasiko eval harvest --repo ~/src/my-repo [--since 2026-11-01]
nasiko eval rewrite
nasiko eval spec detect --repo ~/src/my-repo      # writes specs/<repo>.json; edit if needed
nasiko eval build [--runs 3]
nasiko eval screen
nasiko eval review                                  # only if some task has no verdict
nasiko eval freeze --name my-repo
nasiko eval run --dataset my-repo --model flint [--attempts 1] [--workers 2]
nasiko eval report <run-id> [--format text|json|md]
nasiko eval costs <run-id>                          # re-read costs after adding a price
nasiko eval tasks [--status verified] [--json]
nasiko eval export my-repo@1 > my-repo.jsonl        # SWE-bench format
nasiko eval clean                                   # leftover containers and networks
```

## What runs where

* **Agent container**: the task image on a Docker network created with `--internal` (no route
  out). opencode runs `opencode run --model eval/router --auto` there with no API key. Its only
  way out is the gateway.
* **Gateway**: an nginx container on that network and the normal bridge. It forwards `/v1/*` to
  the Nasiko router only, sets `Authorization` to the eval identity's router token (refreshed
  every 8 minutes; tokens last 10), and sets `x-nasiko-eval-attempt` from a per-attempt secret,
  so an agent cannot see the token or tag calls as another attempt.
* **Router**: serves the model pinned on your `eval` identity (one model at a time, so models run
  one after another) and stores each call's tokens and cost with its attempt tag.
* **Grading container**: the same task image with no network: applies the model's patch (test
  files removed), applies the hidden test patch, builds and runs the tests.

Every container runs with all Linux capabilities dropped, `no-new-privileges`, CPU, memory and
process limits, and the disk limit (checked every few seconds; Docker Desktop cannot enforce
`--storage-opt size`).

## Files

Everything lives under `~/.nasiko/eval/`: `tasks/<id>/` (task, source summary, link scores,
gate logs, reviews), `specs/`, `datasets/<name>/v<N>/` (`dataset.jsonl` + `manifest.json` with
its hash), and `runs/<run-id>/` (`run.json`, one folder per attempt with the agent's config,
prompt, session export, patch, grading log and `attempt.json`, plus `preds.jsonl` in SWE-bench's
predictions format and the reports). Full session logs never leave your machine; only the typed
messages around the task are kept.

## Limits and known gaps

* Only Claude Code logs and Cargo (Rust) repositories so far.
* A long session can hold several tasks; the task's cost covers the messages from the one that
  led to the first edit of the commit's files up to the commit.
* Tests are run per affected Cargo package; integration tests that need a database or network
  fail in the sandbox and drop out of the lists.
* Attempt tags live with the router's hot usage rows (90 days).
