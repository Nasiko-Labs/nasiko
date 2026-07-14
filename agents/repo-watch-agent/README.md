# Repo Watch Agent

An A2A agent that reports on GitHub repo activity over a time window (default: last 12 hours),
across one or more repos, and **automatically deep-dives the risky commits**.

For every window it: lists commits, diffs, and PR activity; identifies which commits touch
risky areas (auth, secrets, `*.sql` migrations, dependency manifests, Dockerfiles, CI
workflows); and for *those* commits only, reads the changed files' full before/after
(line-by-line, not just diff hunks) to report exactly what changed and lists other files
likely impacted — each backed by evidence (a code-search reference to a changed symbol, or a
structural rule of the repo). Non-risky commits get the lighter window summary.

> What changed in the last 12 hours?
> What changed in Nasiko-Labs/nasiko-cloud-rs and Nasiko-Labs/nasiko-rs since 2026-07-13T10:00:00Z?
> Analyze the latest commit in Nasiko-Labs/nasiko-cloud-rs, file by file, and what it impacts.

When no repo is named, it falls back to the space-separated watch list in `GITHUB_REPO`.

All operations are **read-only** (HTTP GET) — the agent cannot modify a repo even if its token
could.

## Config

See `.env.example`. `GITHUB_REPO` is a **space-separated** watch list used when a query names no
repo. `GITHUB_TOKEN` needs read access to the target repos (a classic PAT with the `repo` scope,
or a fine-grained PAT with `Contents: read` + `Pull requests: read`).

## Build & run

```sh
just build
docker run --rm -p 8000:8000 --env-file .env <user>/repo-watch-agent
```

## Deploy

```sh
just release
nasiko deploy <user>/repo-watch-agent --name repo-watch-agent --port 8000
nasiko secrets set OPENAI_API_KEY <key> --agent repo-watch-agent
nasiko secrets set GITHUB_TOKEN <token> --agent repo-watch-agent
nasiko secrets set GITHUB_REPO Nasiko-Labs/nasiko-cloud-rs --agent repo-watch-agent
nasiko restart repo-watch-agent
```
