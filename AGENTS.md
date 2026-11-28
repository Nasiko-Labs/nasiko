# Team notes for coding agents (Codex, Claude Code)

Hackathon team file: Kuheli (kuheli7) + Saurabh (saurabhburnwal). Talk-N-Ops Buildathon, 28 Nov 2026.
**Delete this file before opening the final PR to Nasiko-Labs/nasiko.**

## What we are building: "Model Switch Verdict"
Replay evals already report each model's resolved rate. With 5–10 tasks those ranges are
useless (the old bootstrap gives 5/5 → [100%, 100%]). We answer the real question:
*is it safe to switch from the control model to a cheaper one?*

- Pair the first attempt of both models on the same task (repeats are not new evidence).
- Drop tasks where either side had `infra_error`, and count them. Count unpaired and flaky tasks.
- Newcombe paired-difference 95% range (built from Wilson intervals). It stays honest at small n.
- Margin δ (default 10 points): **SAFE** if the whole range is above −δ, **NOT SAFE** if it is all
  below, otherwise **NOT ENOUGH DATA**, plus "about N more paired tasks would settle it".
- `nasiko eval verdict <run>` exits 0 = SAFE, 2 = NOT SAFE, 3 = NOT ENOUGH DATA, 1 = tool error.
  A CI job can use that exit code to block a bad model switch (this is the bonus point).

## Done (branch `model-switch-verdict`)
- `evals/src/verdict.rs`: pure logic plus 8 unit tests (`cargo test -p nasiko-evals verdict`).
- `cli/src/commands/eval/verdict.rs`: local and cluster runs, exit codes, 3 tests.
- `cli/src/lib.rs`: `EvalVerdictArgs`, `nasiko eval verdict` and `nasiko eval local verdict`.
  Flags: `--candidate`, `--control`, `--margin <points>`, `--allow-inconclusive`, `-j`.

## Open tasks (pick one, say which in the team chat)
1. Replace the percentile bootstrap in `evals/src/report.rs::rate()` with Wilson
   (`verdict::wilson`). Keep `ranges_are_repeatable_and_contain_the_value` passing.
2. Add the verdict line to the markdown report (`cli/src/commands/eval/report.rs::markdown`).
3. A GitHub Actions example in `docs/EVALS.md` that runs `nasiko eval verdict` as a gate.
4. Write-up and demo script (one page, demo of 3 minutes or less).
Server-side changes (`server/src/evals/attempt.rs`) are last: they need a slow Docker rebuild.

## Rules
- The crate name for the CLI is `nasiko` (`cargo test -p nasiko`), and the evals crate is `nasiko-evals`.
- Before pushing, run `cargo fmt`, then `cargo clippy -p nasiko-evals -p nasiko --all-targets`,
  then the tests. New code must have zero warnings. (`Stdio` unused in `coding_agent_router.rs`
  and `indicatif` unused were already there and show only on Windows. Leave them.)
- Cargo rewrites `Cargo.lock` (~8k lines). Revert it with `git checkout -- Cargo.lock` unless
  you really added a dependency.
- Don't commit `.env`, API keys or session data. No AI attribution lines in commits or PRs.
- Work on a branch and open PRs to `kuheli7/nasiko`. Pull before you start, because two people
  are editing.
