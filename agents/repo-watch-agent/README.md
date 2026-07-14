# Repo Watch Agent

An A2A agent that reports on GitHub repo activity — new commits, file-level diffs, and PR
activity — since a given point in time, with an LLM-generated summary and risk flags for
security/config-sensitive changes.

Ask it things like:

> What changed in the repo since 2026-07-13T10:00:00Z?

It defaults to `GITHUB_REPO` when no repo is named in the query.

## Config

See `.env.example`. `GITHUB_TOKEN` needs read access to the target repo (a fine-grained PAT with
`Contents: read` and `Pull requests: read` is enough).

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
