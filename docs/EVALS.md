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
| Dataset | You pick sessions. For each, the server lists the commits made during it (and up to two hours after) on every branch it worked on and the default branch, through your connected GitHub account, and links the session to the commit (or unbroken run of commits) that is its work. A long session whose matching commits are not one short run gives one task per commit, each from the request that led to it. The commit is split into the fix (gold patch) and its tests (test patch), Rust inline `#[cfg(test)]` modules included. A commit that changes no test, or only shared test helpers (`tests/common/mod.rs`), gives no task (`no_tests`): nothing could show the work was done. |
| Rewrite (P1) | A model rewrites the session's request as an issue. |
| Build and gate | The server builds an image per task (the repository at the commit before the fix, as a fresh one-commit repository, with opencode at the OpenCode harness image's version) and runs the tests several times: old code, old code + new tests, fix + new tests. This fills `FAIL_TO_PASS` and `PASS_TO_PASS` and drops flaky tests. Only the packages the task changes are built, and only the integration tests it changes when they are all one package's; a task no Cargo package owns is rejected rather than built as the whole workspace. |
| Screen (P3) | Reviewer models judge each task the way SWE-bench Verified did: is the request clear, would the tests accept any correct fix, how long would it take. A task is kept only if every score is 0 or 1 and nothing is flagged; a task no reviewer could judge waits for a person (Approve or Reject beside it on the Evals page, or `nasiko eval review`). A person's verdict overrides the models', so a task the reviewers rejected after the gate kept it can still be approved (Approve beside it). |
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
| `GET /api/evals/tasks/{id}`, `POST /api/evals/tasks/{id}/review` | A task in full; a person's verdict, on any task the gate found reproducible (`reviewable`) |
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
  process and disk limits and a wall-clock limit (45 minutes per attempt). When an attempt is
  stopped for a limit, its changes are saved first and graded.
* Memory is the repository's build setting `memory_mib` (default 6144, editable with
  `PUT /api/evals/specs/{owner}/{repo}`), the same for the gate, the agent and the grader. A large
  workspace can need more: cargo links two test binaries at once, and with a big dependency tree
  each link can take 4-5 GiB; a linker killed for memory makes the gate reject the task as
  `gate: GoldDoesNotBuild` (or `gate: BaseDoesNotBuild`, when the old code's own build was the
  one killed). On Kubernetes the pod requests the whole limit, so raise it only for the
  repositories that need it.

## Build settings (the spec) and the gate's reasons

A repository's spec says how to build and test it: `toolchain_image`, `system_packages`,
`setup`, `build_cmd`, `test_cmd`, `test_paths`, `timeout_secs`, `memory_mib`, `cap_lints`.
It is proposed on first use and yours to edit (`PUT /api/evals/specs/{owner}/{repo}`; locally
the file `specs/<repo>.json`).

**Lints are capped at warnings (`cap_lints`, on by default).** The same toolchain image is
used for every commit of a repository, and a newer compiler objects to things an older
commit could not have known about, so code nobody touched stops compiling. Measured on
`hyperium/http` with `rust:1-trixie`: of 20 tasks from its commit history, 9 passed the gate
and 9 were rejected because nothing compiled, patched or not. Those nine failed on lints, in
two ways: new warnings under the crate's own `#![deny(warnings)]` (`use of deprecated
constant std::u16::MAX`), and, first in five of the nine, a lint that newer compilers deny
by default with no `deny` attribute in the code at all (`implicit autoref creates a
reference to the dereference of a raw pointer`). The cap turns both kinds into warnings.
With lints capped by a hand-edited spec that set `RUSTFLAGS=--cap-lints=warn` in both
commands, all 9 passed with no flaky test: 18 of 20.

That figure was measured with the hand-edited spec, not with the setting described here.
The setting (a variable in the image, below) has not been run in Docker; only its effect on
cargo was checked, on a small crate with cargo 1.100. Quote 18 of 20 as "with lints capped
by hand" until the gate has been run again with this setting.

* *How.* The repository's images set `CARGO_BUILD_RUSTFLAGS=--cap-lints=warn`. It is in the
  image, not in the commands, so the warm build, the gate, the agent's own `cargo` runs and
  the grader all compile with the same flags; flags that differ between them make each side
  compile from scratch, and a cap only in `test_cmd` would leave the agent's `cargo build`
  failing on old lints. The agent's prompt shows the plain commands (`cargo test -p … --no-run`).
* *What it changes.* A lint that would stop the build becomes a warning: `deny(warnings)`,
  `deny(...)` attributes, and lints the compiler itself treats as errors by default. This
  holds for a model's fix as much as for the old code: a fix that trips such a lint builds
  here, the agent sees only a warning, and the project's own CI might refuse it. The report
  does not flag this. The cap is on for every Rust repository, also those that built without
  it. Type, borrow and name errors are not lints and still fail the build. Lints do not
  change the compiled code, so a test that passes without the cap passes with it. Not
  covered: doc tests (rustdoc does not read this variable) and tests that compile code and
  expect a lint error (`trybuild` and the like), which can now see a warning instead.
* *When it does not apply.* Cargo reads rustflags from one source only: `RUSTFLAGS` (or
  `CARGO_ENCODED_RUSTFLAGS`) in the environment, else `target.<triple>.rustflags` or
  `target.<cfg>.rustflags` in the repository's `.cargo/config.toml` (a
  `[target.'cfg(…)']` table, common for linker flags), else `build.rustflags`, which is the
  list the cap is added to. A repository that sets flags per target, or a spec whose
  commands set `RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS` or `--config build.rustflags=…`, is
  therefore not capped, and nothing at build time says so: its old commits are still
  rejected as `BaseDoesNotBuild` with a lint as the first error. Cargo older than the one
  checked may also replace the repository's `build.rustflags` with the variable rather than
  add to it; this was not tested.
* *Turning it off.* Set `"cap_lints": false` in the spec. The images are rebuilt, because
  their names include their recipe.
* *Specs saved earlier.* A spec without the setting reads as capped, so nothing needs
  editing: the next `build` makes new images with the cap. The same holds on the server: a
  spec sent to `PUT /api/evals/specs/…` without `cap_lints` is saved as capped. A task
  already gated keeps the image and the grading script it was gated with until it is built
  again.
* *A spec capped by hand.* If `build_cmd` and `test_cmd` carry `RUSTFLAGS=…`, remove it: it
  replaces the repository's own `build.rustflags`, and the agent's plain `cargo` runs
  compile with other flags. Then **build every task gated under it again**
  (`nasiko eval local build --only <ids>`) and freeze again any dataset made from them,
  before the next `run`. `run` shows the agent the commands of the spec as it is now, but
  uses the image and the grading script saved with each task. Skip the rebuild and the agent
  is told to run plain `cargo test …` in an old image without the cap, where the untouched
  code does not compile, while the grader still runs the old capped script. Nothing warns
  about this state.
* *`spec detect` on a repository that already has a spec* keeps the saved one. It prints the
  settings in which it differs from today's proposal and whether a command mentions
  `RUSTFLAGS` (a plain text check: any mention counts). A better proposal, or a
  `rust-toolchain` file changed since, therefore does not reach a saved spec by itself;
  delete `specs/<repo>.json` to start again from the proposal. Specs are filed by repository
  name alone, so `a/tool` and `b/tool` share one file: the command says so when it meets the
  other one's spec, keeps its settings and renames it. A spec file that cannot be read
  stops the command; it is not overwritten.

**Why the gate rejected a task.** The reason is saved on the task as `reject_reason`, in
the form `gate: BaseDoesNotBuild` (the server's stored gate verdict names the same reasons
in snake case: `base_does_not_build`):

| `reject_reason` | What the gate saw | What to try |
|---|---|---|
| `gate: BaseDoesNotBuild` | The build command fails on the old code in the task image with no patch applied, so the fix was not judged. Also given when that run was stopped at the time or disk limit. | Read the first error. Pin an older `toolchain_image` (`rust:1.75-bookworm`, say); if the error is a lint, see "When it does not apply" above; raise `memory_mib` if the build was killed, `timeout_secs` if the run was stopped for time. |
| `gate: GoldDoesNotBuild` | The build command works on the old code and fails on the real fix with its tests. Also given when a run of the fix was stopped at a limit. | Read the first error; raise `memory_mib` if the build was killed, `timeout_secs` if the run was stopped for time. |
| `gate: NoFailToPass` | No test goes from failing to passing. | Nothing: no test would show the task was done. |
| `gate: AllFlaky` | Every test that could prove the fix changed result between identical runs. | Nothing, unless the tests can be made steady. |

These are what the gate saw in one image, not a finding about the code. `BaseDoesNotBuild`
is not a count of toolchain trouble: it also holds runs that ran out of time or disk and
builds killed for memory.

A base that does not build is reported as such whether or not the fix builds on top of it;
the fix is named only when the old code builds without it. Locally, `build` also prints
each rejection in words and, for a failed build, its first compiler error and the log's
path:

```
  http__03c8cd7f: rejected (BaseDoesNotBuild): the build command fails on the old code in this image with no patch applied, so the fix was not judged. Read the first error. Often it is an old commit on a newer compiler: pin an older toolchain_image in the spec. If the error is a lint, the lint cap is not reaching the compiler: check that the spec has cap_lints on, that its commands do not set RUSTFLAGS, and that the repository's .cargo/config.toml sets no target rustflags (those switch the cap off). If the build was killed, raise memory_mib
    pristine-1: error: use of deprecated constant `std::u16::MAX`: replaced by the `MAX` associated constant on this type
    log: ~/.nasiko/eval/tasks/http__03c8cd7f/gate/pristine-1/output.log
```

(The lines are what the code prints for that failure; the example was put together from a
unit test, not copied from a Docker run.) What the local `build` does with a failed run:

* The error line is printed, not saved: `reject_reason` holds the reason's name only, the
  same string locally and on the server, so tasks can be counted by it.
* When the fix fails to build for another cause than the base, both first errors are shown.
* A compiler killed by a signal is shown with the signal
  (`error: could not compile … (signal: 9, SIGKILL: kill)`), the usual sign of too little
  memory.
* A run stopped at the time or disk limit keeps the reason's name but is described as
  stopped, with the setting to raise, and not as code that does not compile.
* A build that failed with no `error` line in its log says so.
* A task's test scope comes from the commit of the fix, so it can name a test target
  (`--test <name>`) or a package that the old code does not have yet. Cargo then refuses
  the untouched run before compiling anything (`no test target named …`). The local gate
  reads that as "these tests did not exist before the fix", not as a base that does not
  build. This is a text check on cargo's message (seen with cargo 1.100, not with older
  ones). When the old code with the new tests does not compile either, every test that
  passes after the fix is then listed as FAIL_TO_PASS, also one from a test target that
  did exist and passed before; a model's patch must make the same tests pass either way.
  The server's gate does not do this yet: there such a task is rejected as
  `BaseDoesNotBuild`, and no error line is shown.

## On your own machine: `nasiko eval local`

For repositories that are not on GitHub, `nasiko eval local …` runs the same pipeline on your
machine: local Claude Code logs, local git and Docker, files under `~/.nasiko/eval/`
(`NASIKO_EVAL_HOME`). `nasiko eval local init` creates `config.json` (models by router provider
and model, rewriter, reviewers, control, limits); then `harvest --repo`, `rewrite`,
`spec detect`, `build`, `screen`, `review`, `freeze --name`, `run --dataset --model`,
`report`, `costs`, `export`, `clean`. Its model calls still go through the router, as your
`eval` identity, one model at a time.

### From a repository's commit history

A repository with no Claude Code sessions (a team that has none yet, or any public Cargo
repository) can still give tasks: `harvest-commits` reads the repository's own history instead
of session logs. Only this first step differs; every later step is the one above.

```sh
export NASIKO_EVAL_HOME=~/nasiko-eval-bytes     # its own folder: build and freeze take every task in it
nasiko eval local init                          # then add models, "rewriter" and "control" to config.json
nasiko eval local harvest-commits --repo ~/src/bytes --since 2025-01-01 --dry-run
nasiko eval local harvest-commits --repo ~/src/bytes --since 2025-01-01 --max 25
nasiko eval local rewrite                       # required: freeze refuses a task without it
nasiko eval local spec detect --repo ~/src/bytes
nasiko eval local build
nasiko eval local screen                        # then `review` for tasks no reviewer model judged
nasiko eval local freeze --name bytes
nasiko eval local run --dataset bytes --model <short name> --model <short name>
nasiko eval local report <run>
```

**Which commits.** Those reachable from `HEAD`, newest first, each on its own. Other branches
are not read: they hold backports and unmerged work, which would turn one fix into several
tasks. `--since` leaves out commits made before that date (the commit date; every commit from
that date on is kept, also one that sits behind an older commit). `--max N` stops going back
once N commits are usable tasks: tasks of an earlier run count, rejected ones do not, so the
same command run again after `rewrite` rejected some goes further back and replaces them.
Every commit gets exactly one outcome, printed as a funnel. A dry run on a 200-commit shallow
clone of this repository:

```
nasiko: 200 commits reachable from HEAD
  commits scanned                     200
    merge or root commit               29
    too many files                      8
    no Rust code change                58
    no tests                           29
    too thin a message                  0
    unreadable                          0
    would become a candidate task      76  (61 with a detailed message)
```

The command says when a clone is shallow: its oldest commit then counts as a root commit (one
of the 29 above), and nothing older is listed. With `--since` the first line ends in
`since <date>`.

| Outcome | Rule |
|---|---|
| merge or root commit | Not exactly one parent: a merge's diff is not one change, and a root commit has no code to start from. Also a commit with one parent whose subject is the one git writes for a branch merge (`Merge branch '…'`, `Merge remote-tracking branch '…'`, `Merge tag '…'`): a flattened history keeps such merges, and their diff is a whole branch (28 of the 29 above). |
| too many files | More than 40 changed files, the limit a session's commit has too. |
| no Rust code change | Once the diff is split into fix and tests (the same split as for sessions, inline `#[cfg(test)]` modules included), the fix has no `.rs` file: the commit changes only tests, documentation, manifests, other languages or binary files, or nothing. |
| no tests | It adds nothing to a test: it has none, changes only shared test helpers (the `no_tests` rule above), or only removes tests, as a revert does. |
| too thin a message | It has a fix and tests, but its message has fewer than five words (and fewer than ten characters of a script written without spaces, such as Chinese) once trailers, a generated `Merge …` subject and bare numbers such as `#712` are left out ("fix bug", "Fix #712"). Trailers are lines like `Signed-off-by: …` or `Fixes: #712` in the last paragraph; a line such as `Before: total(0.5) returned 0` is kept. |
| task id taken | Two commits share their first 8 sha characters; the second is skipped (the row is shown only when it happens). |
| candidate task | Everything else. Its id is `<repo>__<first 8 characters of the commit sha>`. |

`--dry-run` prints the funnel and the commits that would become tasks, and writes nothing: use
it to see whether a repository has enough of them before building anything.

**A candidate is not yet a usable task.** The count is an upper bound. `rewrite` rejects
messages that state no problem, and `build` drops a task whose tests do not fail before the
fix and pass after it. The count of detailed messages (150 characters or more of what the
author wrote, so without trailers and a generated merge subject) is a first guess at how many
get through `rewrite`. Some candidates are not one change either: of the 76 above, 8 are
merged pull requests (`Merge pull request #… from …`, kept, because a pull request is usually
one change) and 4 are hand-written syncs (`Merge development into …`, `sync: …`) that only the
rewriter can turn down.

**The statement is the risk.** A commit message is the only description there is, and it often
describes the fix, which would hand the answer to the model being tested. So:

* the task is saved with the raw message as its statement and marked as from commit history
  (`x_nasiko.source: "commit-history"`, `statement_rewritten: false`);
* `rewrite` gives the rewriter model the commit message and nothing else (no diff, no changed
  file names, no trailers) and asks for the problem and the expected behaviour without the
  solution. It answers `REJECT`, which rejects the task, when the message only says what was
  changed and not what was wrong, or describes a merge or several unrelated changes;
* a rewritten statement that repeats the commit message, or names a file the commit changed,
  is not accepted: the task stays unrewritten and `rewrite` reports the error;
* `screen` and `review` leave a task out, and say so, until it is rewritten: a verdict is
  always on the statement the model will be shown;
* `freeze` refuses to write a dataset while a verified task from commit history has not been
  rewritten, and names the tasks.

What this does not prove: a statement can still describe the fix in other words. The two
checks above catch only an echo and a file name; beyond them the reviewer models'
`statement_leaks_solution` flag in `screen` (a flagged task is rejected) and a person reading
the statement are what stand between a leaking statement and the dataset. The rewrite prompt
for commit messages is new and has not yet been run against a model or measured on a labelled
set, so read the first statements yourself (`nasiko eval local tasks --json`). The message is also a stranger's text in a public
repository: it is quoted to the rewriter as data and cannot close its own quote, but a model
can still be talked round, which is one more reason to read them.

Run `rewrite` before `build`: it is one model call per task, and a task it rejects costs no
image build. Expect to lose a good share here: that is the price of not leaking answers.

**Running it again** adds nothing: each commit's outcome is kept in the repository's harvest
ledger (`harvest/<repo>.json`, beside the sessions'). `--redo` looks at every commit again; a
commit whose task already exists, from an earlier run or from a session that ended in it, is
left as it is, so a task keeps its gate results, its rewritten statement or its rejection.
The other way round, `harvest` (from sessions) rejects the commit-history task of a commit
that one of its sessions produced and says so: the session's task has the real request and
the real cost, and one commit is never two tasks of a dataset.

**What a commit-history task does not have.** There was no session, so there is no link
score, no turn count, no original model and no original cost: `tasks` shows `-` for LINK and
TURNS, while the task file keeps stand-in values (`link_score: 1.0`, one turn, an empty model)
to fit the format. Set `control` in `config.json` so that the report compares the models
with each other. Without a control, or when the control solved no task, the report's
headline still reads "matched the original sessions on N% of tasks" and compares cost with
"the original sessions": for these tasks read that as "passed the hidden tests on N% of
tasks", and ignore the cost comparison (there is no original cost).

## Limits and known gaps

* Claude Code sessions and Cargo repositories only (`nasiko eval local harvest-commits` needs
  no sessions, but runs only on your own machine: the server and the UI cannot build a dataset
  from commit history yet).
* A task from commit history has no original session, so no original cost and no original
  model: set `control` in `config.json` so the report compares the models with each other
  (see "What a commit-history task does not have" above for what the report says otherwise).
* The fix of a commit-history task must change a `.rs` file, and the commit must add to a
  test; a pull request merged as several commits is several candidates, not one.
* For a public repository the real fix may be in a model's training data. Read such a run as
  a comparison between models, not as an absolute rate.
* A session is usable only if it was synced with chat history, and after its repository was
  recorded (re-sync older sessions to backfill it).
* Tests run per affected Cargo package; integration tests that need a database or network fail
  in the sandbox and drop out of the lists.
* One toolchain image is used for every commit of a repository: the version pinned in
  `rust-toolchain(.toml)` at `HEAD` when the spec was proposed, else the newest stable.
  Choosing a toolchain per task from its commit's date is not done. Not checked: whether
  `rustup` inside the image fetches another toolchain when a task's own commit carries a
  `rust-toolchain` file (if it does, that happens only while the image is built, the one
  step with network). Capping lints (`cap_lints`) covers the breakage seen on old commits of
  one crate, lints a newer compiler raises or denies; it does not help when an old commit
  fails on a newer compiler for another reason (a dependency that no longer compiles, a real
  language change). Such a task is rejected as `gate: BaseDoesNotBuild`, and pinning an
  older `toolchain_image` for the whole repository is the only remedy today.
* A pinned version is proposed as `rust:<version>-trixie`. Whether that image exists for an
  old version was not checked; if the image build fails, set `toolchain_image` by hand.
* The lint cap as built (a variable in the image) has not been through a Docker gate run;
  the 18 of 20 on `hyperium/http` was measured with the cap written into the commands by
  hand. The cap is silently off for a repository whose cargo config sets target rustflags.
* With lints capped, a model's fix that trips a lint the project or the compiler treats as
  an error still builds and can pass; the report does not flag it.
* After a spec's commands are edited, tasks gated before the edit keep their old image and
  grading script while the agent is shown the new commands; `run` does not detect the
  mismatch. Build those tasks again and freeze the dataset again.
* The server's gate reads a pristine run less carefully than the local one: a test target
  the old code lacks is rejected there as `BaseDoesNotBuild`, and the server prints no
  first error line.
* Few sessions end in a single clean commit, so a meaningful score needs many sessions.
* Attempt tags live with the router's hot usage rows (90 days).

## What the run report checks beyond pass or fail

`nasiko eval local report <run>` (text, `--format md` and `--format json`) answers more than
"how many tasks did the model solve". Every section below is worked out when the report is
printed, from files the run already left under `runs/<run>/attempts/<task>/<model>/<n>/`
(`attempt.json`, `out/raw.patch`, `model.patch`, `out/session-export.json`) and the frozen
dataset, so it also works on runs made before these sections existed and it changes nothing
about how a run is made or graded. When a file is missing or cannot be read, the attempt is
left out of that figure or a stated fallback is used, and a "Not read" list at the end of the
report says which and why. In JSON the new keys are `consistency`, `tasks`,
`sandbox_failed_first_attempts` and `signals`; `run`, `summaries` and `attempts` are unchanged.
These sections are in the local report only: the server's report shares the resolved rate and
its range (the same code), not the sections that read the patch and the session.

The report reads in this order: the headline and the model table; **How it worked** (how each
agent went about the task, read from its session); **What it produced** (works every time,
scope against your fix); the per-task table with one line per failed attempt and one line of
sentences per attempt; failure reasons and the by-difficulty table; test tampering as a closing
note; "Not read". The pass rate says whether the hidden tests passed. "How it worked" is for
the team that is deciding whether to hand work to a cheaper model and wants to know whether it
investigated or guessed. Every table there has a row for all attempts whose session was read,
one for those that resolved their task and one for those that did not, so a habit that goes
with failure shows. They are signals about method, not verdicts: a correct change can come out
of a session that reads badly, and the other way round.

A session records only part of what an agent did, so these sections keep three answers apart:
the session shows it, the session shows the opposite, and *cannot tell*. An attempt that cannot
be told is counted in its own column and left out of the share; it is never counted as "did
not".

Some words are used the same way in all of them:

* The agent *opened* a file when its read tool, a shell command that shows files (`cat`,
  `head`, `tail`, `sed -n`, `wc`, `git diff -- file` and the like) or a search was pointed at
  it by name, or when it edited it.
* A search *listed* a file when the output of a contents search the agent ran over files (its
  search tool, `grep -r`, `rg`, `git grep`), or of `git diff`, `git show` or `git log`, names
  it. A `grep` that only filters another command's output (`cargo build | grep error`) is not
  a search of the code, and the output of a build or test run never counts.
* A command *named* a file when it writes the path out in full without being a program known
  to show files: a script given the path, `ls`. Commands that only change or remove a file
  (`rm`, `mv`, `git checkout`, `sed -i`, a redirect into it, an `echo` that mentions it) do
  not count at all.
* *Cannot tell*: a command may have shown the file without naming it. A viewer or search
  pointed at a glob that covers it (`cat src/*.rs`), fed by `$(…)`, backticks or `xargs`, or a
  script handed to an interpreter through a heredoc.
* An *edit* is a call of one of the agent's edit tools (`edit`, `write`, `apply_patch`). A file
  *was already there* when a patch (the attempt's or your fix) changes it, when the edit tool
  says so, or when the agent's read tool opened it before the edit. A file is *the agent's own*
  when its patch adds it or the edit tool says it made it; later edits to it are edits to its
  own file. Where nothing says (a whole-file write the session does not mark, to a file that
  is in no patch), the file is taken as a scratch file, not as existing code.
* A *build or test command* is `cargo test`, `nextest`, `build`, `check` or `clippy` (also as
  `cargo t`, behind `cd x &&`, `timeout 60`, `VAR=value`, `uv run`, `poetry run`, `npx`, or
  inside `bash -c "…"`), `pytest`, `python -m pytest`, `python -m unittest`, `tox`, `nox`,
  `go test`, `go build`, `go vet`, `npm test`, `npm run test`, `yarn test`, `pnpm test`,
  `bun test`, `jest`, `vitest`, `mocha`, `make test`, `make check`, `mvn test`, `gradle test`,
  `dotnet test`, `ctest`, `rspec`, or the spec's own build or test command. A command line
  that only mentions one (an `echo`, a commit message, a `grep`, a heredoc body, `--help`)
  does not count, nor does one that only lists tests (`pytest --collect-only`, `tox -l`).
  `cargo test --no-run` and `cargo test -- --list` compile the code and count as a build. A
  command that did not start (exit status 126 or 127, or "command not found" in its output) is
  not a run.

| Section | What it tells a team | How it is computed |
|---|---|---|
| Resolved (95% range) | How sure the number is. Ten of ten is "100%, somewhere between 72% and 100%", not "100% to 100%". | A Wilson score interval. Each task counts once, by its first attempt, in the resolved rate, the match rate and the by-difficulty table; three attempts on ten tasks are ten tasks, not thirty. Cost and cost per resolved still cover every attempt (all attempts' cost divided by the attempts that resolved), and so do the failure counts. A task whose first attempt failed on the sandbox (`infra_error`) counts as not resolved; the report names those tasks under the model table so a flaky sandbox is not read as a weak model. |
| How it worked: where it looked | Whether the agent ever looked where your team's fix is. "No look shown" and "looked but did not change it" are different failures: the first is about finding the place, the second about what to do there. | For each file your fix changed that was already there (files your fix adds, test files and lock files are left out): the attempt's final patch changes it too; or the agent looked at it (opened it, a search listed it, or a command named it) and left it unchanged; or a command may have shown it (cannot tell); or the session shows no look at it. Files it looked at without ever opening them are counted apart, because one broad search lists many files. Per model: the attempts compared, the share of them that looked at every such file (attempts that cannot be told left out), and how many files fall in each group. |
| How it worked: searched for what it changed | The nearest a session comes to "checked who uses a thing before changing it". | A shared item is a `pub` function, struct, enum, trait, constant, static, type alias or field (`pub(crate)` too) in a Rust file that is not a test, whose declaration the attempt's graded patch removes or changes. The declaration is its line up to the body (`{`) or the end (`;`), white space apart, so a line that only moved, was reformatted or changed in its body is not counted; an item is counted per file. "Searched for" means that at any point of the session the agent ran a contents search whose pattern holds the item's name as a whole word and that reached beyond the file declaring it (its search tool, or `grep`, `rg`, `ag`, `ack`, `git grep` over files). Per model: the patches with Rust code (the ones read), the attempts that changed a shared item, the share of them that searched for the name of every one, and the items changed and searched for. |
| How it worked: how widely, and when | Whether it explored before it started editing, or edited first and looked later. | Distinct files opened by name and distinct files edited, as medians per model, and the share of its looking steps (opening, listing, searching) that came before its first edit. |
| Ran the build or tests before changing code | The nearest a session comes to "saw the problem before changing anything": whether it ran anything before changing code, and whether what it ran failed. | Whether a build or test command ran before the agent's first edit to code that was already there. Not that first change: writing or adjusting a file of its own (a reproduction), a file outside the repository, an edit under a test path, and an edit that removes nothing and adds only Rust tests (every item it adds stands behind `#[test]`, `#[tokio::test]`, `#[rstest]`, `#[cfg(test)]`). A whole-file write over an existing file is always a change. "Showed a failure": the run was the last command of its line and the line's exit status is a failing one; or, where the status is another command's (a pipe, `;`, `||`, `&`, `$(…)`), the output the session kept holds failure text (`FAILED`, `--- FAIL`, a line starting with `error:` or `error[E…]:`, a Python traceback, "1 failed"). A run stopped at a time limit (exit 124, 137, 143, or opencode's own timeout note) is "cut off", neither. |
| Guessed and redid | How directly it got to its change: written once, or written, thrown away and written again. | Lines added over all the agent's edits to files inside the repository that are not test files, divided by the lines its patch as handed in adds to such files. A test written inside a source file is in both counts (grading undoes it, so the handed-in patch is used, not the graded one). 1.0 is "wrote it once". An attempt that wrote lines and kept none has no ratio; it is counted in its own column and together with those above 2.0. Per model, over the attempts that edited with edit tools: the median ratio, the share above 2.0 or with nothing kept, the attempts that kept nothing, and the attempts that edited a file that is not in the patch they handed in. |
| Tested its own fix before saying done | Whether the agent ran the build or the tests after its last edit, or handed in unverified work. | The last edit to a file inside the repository and whether a later shell command ran a build or test command. Per model, the share of attempts with edit-tool calls that did, and those that never ran one. Attempts that changed files with no edit-tool call are counted apart as "cannot tell". |
| Read before changing | Whether it edited files it had not looked at. Expect it to read high for every model: opencode's own edit tool tells the model to read a file before editing it. | For each file that was already there and that the agent edited: had it opened the file, or had a search listed it, before its first edit to it. A file a command only named or may have shown is "cannot tell". Files the agent made itself do not count. Per model: the attempts that edited existing files, the share that looked at every such file first (attempts that cannot be told left out), the files edited with no look shown, and the files that cannot be told. |
| Works every time | Whether a model solves a task reliably or only on a lucky run. | Needs more than one attempt per task (`run --attempts 3`). Per model: the share of tasks solved in every attempt, in at least one, and in some but not all, each with its range, and the "sometimes" tasks by id. Tasks with an attempt lost to the sandbox are named, because that attempt counts as not solved. With one attempt per task the section says it cannot tell. |
| Scope against your fix | What a passing change looks like next to the one your team made: same files or others, similar size or several times larger. | The graded patch against the task's real fix, test files and lock files (`Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `go.sum`, `poetry.lock`, `uv.lock`, `Gemfile.lock`) left out of both: files both touched (a file the model also renamed still counts), files only the model touched, files only your fix touched, and added plus removed lines of each. Per model, split into resolved and not resolved: the share that touched a file your fix touched, the median size ratio, the share with extra files, with left-out files, and more than 3x the size. It says "differs", not "wrong": another correct fix can touch other files. Attempts with no code change are left out. |
| Per task | Which tasks each model solves, at a glance. | One row per task, one column per model, each cell the attempts that resolved it out of the attempts made (`2/3`), with `T` when an attempt touched existing tests in a file your commit left alone, `t` when it touched only test files your commit changed too, and `L` when a change was more than 3x the size of your fix. |
| Failed attempts: what the session shows | A likely story for each attempt that did not resolve its task, to check against its session. | One line per failed attempt with the first three of these that apply, each worded as what the session shows: no look at a file your fix changed (or a look at one without a change to it, or one seen only in a search result and never opened); no search for the name of a shared item it changed; no build or test run before its first change to existing code; no look at a file before editing it; more than twice the lines written than kept, or none kept; no build or test run after its last edit. What cannot be told is left out. The line is what stands out, not the reason the attempt failed. |
| Attempt by attempt | What each attempt's session shows, in words. | One line per attempt whose session was read (the first 20; all are in JSON): one sentence per measure above, in the report's order, for example "The real fix changed 2 file(s); it changed 1 of them; its session shows no look at src/uri/path.rs." A measure that could not be read for the attempt has no sentence. |
| Test tampering | Whether the model changed tests that were already there to get to green. A closing note, not part of the score. | Read from the patch as the agent handed it in, before grading drops its test changes. Each attempt gets one verdict. *Removed or disabled tests*: a test file or test function deleted or renamed, a test that lost its test attribute or became a plain function, or `#[ignore]`, `#[should_panic]` or a `#[cfg]` gate put on an existing test or test module. *Edited existing tests*: any other change to existing test code or to an existing file under a test path (fixtures, snapshots), including lines added inside an existing test, with the files named. Otherwise *no change to existing tests seen*. Per model, the share of attempts with each, the assert lines deleted and not replaced in the same changed block, and how many flagged attempts touched only test files your own commit changed too (there the task itself needed the tests updated). New test files and new test functions are listed apart as "wrote its own tests" and are never tampering. |

In JSON every attempt carries its own figures and evidence under `signals.attempts[]`:
`method.fix_files` (the paths in each group: `changed`, `looked_not_changed`, `never_opened`,
`cannot_tell`, `never_looked`), `method.shared_items` (each item, its file, whether its name
was searched for; `null` for a patch without Rust code), `method.breadth`, `method.first_run`
(`ran_before_first_change`, `outcome`: `showed_failure`, `no_failure_seen`, `cut_off` or
`not_shown`, the command, and how many commands did not start), `method.rework` (lines written
and kept, the ratio, whether the graded patch had to stand in for the handed-in one),
`method.blind_edits`, `self_check`, the sentences under `how_it_worked`, and `story` for a
failed attempt. Per model, `signals.models[].method` has the groups `all`, `resolved` and
`not_resolved`, and `self_check_by_outcome` the same split.

Limits of these checks, so nobody reads more into them than is there:

* "How it worked" reads the session opencode exports after a run. The reader was written from
  the opencode release Nasiko pins (the types of its SDK package and the tool code bundled in
  its binary), and has not yet been run on an export from a real replay; the report says so
  above its tables. A session it cannot make sense of is listed under "Not read" and left out,
  never read as "did nothing": one that is not JSON, one where no tool call could be read, one
  whose shell calls or whose edit calls all lack the input they need, and one that shows no
  step at all although the attempt changed files. A search call it cannot read next to others
  it can is counted as an unread call, and a call cut off while it ran is never taken as an
  unknown format.
* It sees only what the session records, and a shell command is read by its words, not run. A
  file the agent read through a script that does not name it reads as "no look shown". "Looked
  at" is generous on purpose (a search that listed the file counts), which is why the files
  never opened are shown next to it: one `grep -rn 'fn ' .` lists every file. A file the agent
  made with a shell command and then edited reads as a file that was already there.
* A file changed by a shell command (`sed -i`, a redirect) is not an edit. An attempt that only
  edited that way is left out of "Ran the build or tests before changing code", "Guessed and
  redid", "Tested its own fix" and "Read before changing"; where it looked and which shared
  items it changed are still read, because those come from its patch.
* Shared items are Rust only: a patch without Rust code is shown as not read, never as
  "changed nothing shared". They are found on the first line of a declaration; a signature
  spread over several lines is seen only when that line changes. A search for the name does
  not show that the search found the users or that the agent read them, and a short name
  (`new`, `len`) is matched by searches made for other reasons.
* "Showed a failure" is not "reproduced the bug". The failure can be about something else, and
  an agent that understood the problem by reading the code reads as "did not run first". When
  a run is behind a pipe only its output is read: output filtered down to nothing
  (`cargo test | grep FAILED` with no match) reads as "the session does not show how that run
  ended". A run that follows `cd missing-dir &&` and never started still reads by the line's
  exit status.
* "Guessed and redid" counts every line of a whole-file `write`, so a model that rewrites files
  in full reads high, and scratch files written inside the repository count as written. Under
  1.0 means part of the change was made another way. When an attempt left no raw patch, the
  graded patch stands in for the lines kept, and a test written inside a source file then
  reads as written and not kept ("Not read" says for how many attempts).
* "Read before changing" says more about opencode than about the model: its edit tool tells
  the model to read a file first, and an edit the tool turned down is not among the session's
  steps, so it is not counted as an edit without a look.
* The split by resolved and not resolved shows which habits go with failure in this run. With a
  handful of attempts in a group every range is wide (one attempt of one reads "100%, somewhere
  between 21% and 100%"): that is a reason to open those sessions, not a finding.

* Test tampering reads the diff by Rust's conventions (`#[test]`, `#[tokio::test]`, `#[rstest]`,
  `#[test_case]`, `#[ignore]`, `#[should_panic]`, `#[cfg(..)]`, `assert` macros). It reads text,
  not meaning: an assert swapped for a weaker one or a changed expected value is "edited
  existing tests" with the file named, for a person to judge. "No change to existing tests
  seen" is not proof that nothing was weakened.
* In a normal source file a change counts as a test change only where the diff shows it is in
  test code: from a test attribute or `#[cfg(test)]` to the end of that item, or under a hunk
  heading that names a `tests` module. A change deep inside a long inline test module whose hunk
  shows neither is missed.
* Grading drops test files from a model's patch. Its edits to tests inside source files are
  undone only when the spec has a `local_path` to read the original from; otherwise they are
  graded as handed in.
* Test paths and the spec's own commands are read from the spec as it is on disk when the
  report is printed, not as it was when the run was made. Changing `test_paths` changes these
  sections for old runs too.
* Scope compares files and line counts, not meaning. A larger change can be the better one.
* "Tested its own fix" does not know whether the command passed, and does not recognise a test
  script or `make` target of another name unless the spec names it; such an attempt reads as
  "never ran them". Every edit inside the repository counts as an edit, also a note the agent
  wrote there after its last test run. If attempts changed files and no edit-tool call shows,
  "Not read" says so and lists the tool names the sessions did call; if that is every attempt,
  do not rely on "How it worked" for that run.
* `report.json` names, per attempt, the files of the real fix (`signals.attempts[].scope` and
  `.method.fix_files`). Treat
  it like the dataset itself: do not put it where a model under test can read it.
* With few tasks every range is wide. That is the honest answer, not a defect of the report.
