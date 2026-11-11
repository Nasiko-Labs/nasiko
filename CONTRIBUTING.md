# Contributing to Nasiko

Thanks for your interest in Nasiko. This guide explains what we accept, how a pull request is
handled, and the checks a change must pass.

- Documentation: [docs.nasiko.com](https://docs.nasiko.com)
- Questions and discussion: [Discord](https://discord.com/invite/HmnfkTfjFv)
- Bugs and proposals: [GitHub issues](https://github.com/Nasiko-Labs/nasiko/issues)

## Read this first

We welcome contributions, and our review time is limited. Opening a pull request does not oblige us
to review or merge it: we may close or defer it, ask for a smaller scope, or solve the problem a
different way. Following this guide makes a pull request eligible for review; it does not
guarantee acceptance.

Focused bug fixes, reliability and performance improvements, missing tests, and documentation
corrections are the most likely to be accepted. Unrequested features, broad rewrites, and unrelated
cleanups are not.

## Agree on the problem before you build

- **Small, obvious bug fix:** open the pull request directly. Explain the defect and why your fix
  is correct.
- **Larger bug fix:** link an issue that describes the failure and the expected behaviour.
- **New feature or change in product behaviour:** open an issue first and wait for a maintainer to
  agree on the direction and scope. Link that agreement in your pull request. Describing a
  behaviour change as a bug fix does not skip this step.

Search existing issues and pull requests before you start, so two people do not build the same
thing.

## One problem per pull request

A pull request solves one underlying problem. Include everything that problem needs (code, tests,
documentation, migrations) and nothing else. A second fix, a refactor, or a cleanup belongs in its
own pull request unless the first cannot work without it. A large change is fine; an unrelated one
is not.

## Show that it works

In the pull request description, explain:

- how you reproduced the problem,
- what you changed and why,
- the exact commands or manual steps you used to check the change, and what you observed,
- anything you could not check.

"Tests pass" on its own is not evidence. A bug fix needs a test that fails before the fix and
passes after it. A UI change needs before and after screenshots, plus a short recording when motion
or timing matters. Attach them to the pull request; do not commit them to the repository.

## What happens after you open a pull request

1. Your pull request is picked up automatically, and checks run for the parts of the code it
   touches: `cargo test` for the affected crates, and type checking, linting, build and tests when
   you change `ui/`.
2. The result appears on your pull request as a commit status and a comment that is kept up to
   date as you push. If your branch no longer applies cleanly, you are asked to rebase.
3. When several pull requests solve the same problem, maintainers compare how reviewable they are
   (tests, size, how many areas they touch, documentation) and keep one.
4. A merged pull request keeps your original commits and authorship.
5. A pull request that is closed without merging always gets a comment with the reason: superseded
   by another pull request, duplicate, out of scope, failing checks, needs a rebase, or stale.
   Address the reason and you are welcome to open a new pull request.

## Getting started

You need Rust 1.85 or newer (the workspace uses edition 2024), Docker or Podman, and
[`just`](https://github.com/casey/just). UI work also needs Node.js 22.18 or newer and pnpm.

```sh
just infra   # start the backing services
just run     # run the server
```

The full setup guide for every operating system, with environment variables and troubleshooting,
is in the [README](README.md#setup-guides-by-operating-system). For where code lives, see
[Project Structure](README.md#project-structure) and [`docs/ORGANIZATION.md`](docs/ORGANIZATION.md).

## Before you push

All of these must pass with zero warnings:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
just test-unit
```

Then run the integration tests for the area you changed. For changes under `ui/`, run
`pnpm install`, `pnpm typecheck`, `pnpm lint` and `pnpm test` inside `ui/`.

| Command | Needs | Covers |
|---|---|---|
| `just test-unit` | nothing | Hermetic unit tests: no network, database or Docker |
| `just test-crates` | `just infra` | Library integration tests that need Postgres |
| `just test-server` | `just infra` | Server integration tests |
| `just test-one <name>` | `just infra` | One server test file, for example `just test-one auth_flow` |
| `just test` | `just infra` | All of the above |

Unit tests must stay hermetic. Integration tests share one database, so they run one at a time.

## Code standards

The binding guides live in [`docs/`](docs):

- [`CLEAN_CODE_GUIDE.md`](docs/CLEAN_CODE_GUIDE.md): names, function size, error handling, traits
  and tests
- [`ORGANIZATION.md`](docs/ORGANIZATION.md): where code belongs
- [`API_CONVENTIONS.md`](docs/API_CONVENTIONS.md): the wire format of every HTTP endpoint
- [`A2A_PROTOCOL.md`](docs/A2A_PROTOCOL.md): how agents talk to the platform

The rules that most often block a pull request:

- **Zero warnings.** `cargo check` and `cargo clippy` report nothing, and the code is formatted with
  `cargo fmt`.
- **Dependencies are declared once,** in the root `Cargo.toml`; crates use `dep.workspace = true`.
  Explain why a new dependency is needed when an existing one cannot do the job.
- **Extend through traits.** Seams such as `AuthService`, `ContainerRuntime`, `RoutingEngine` and
  `ObservabilityProvider` are wired once at startup, and handlers receive `Arc<dyn Trait>`.
- **The CLI stays lightweight:** synchronous HTTP with `ureq`, no async runtime.
- **Migrations are new files.** Add a file named `NNNN_short_description.sql` in `migrations/`,
  numbered one above the highest existing file. Migrations run automatically at startup; never edit
  one that has already been released.
- **No secrets,** not even in examples, fixtures or tests.

## Reporting bugs

Open an [issue](https://github.com/Nasiko-Labs/nasiko/issues) with:

- what you expected to happen,
- what happened instead,
- the steps to reproduce it,
- your operating system, Rust version, and Nasiko version or commit.

Logs help; remove any secrets from them first.

## Security issues

Do not report a vulnerability in a public issue or pull request. Contact the maintainers privately
on [Discord](https://discord.com/invite/HmnfkTfjFv) and we will follow up with you directly.

## License

Nasiko is licensed under the [Apache License 2.0](LICENSE). By submitting a contribution, you agree
that it is licensed under the same terms.
