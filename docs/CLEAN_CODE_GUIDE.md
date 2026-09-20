# CLEAN_CODE_GUIDE.md

Authoritative engineering reference for the **Nasiko** repository. Both human engineers and AI
coding agents **must** follow this document when writing, modifying, or reviewing code.

Precedence: when a request conflicts with a rule here, follow the rule and say so.

---

## Project overview

Nasiko is an A2A (Agent-to-Agent) orchestration platform: it deploys, manages, and routes between
AI agents using the A2A protocol. This repository is the open-source edition; a separate
enterprise edition **wraps and extends** these crates through traits. That has one hard
consequence for this codebase: there are **no feature flags and no enterprise conditionals**
here — extension happens only through trait seams (see §5).

Setup, build, and run commands live in `CONTRIBUTING.md` (repo root) — this document covers
only the standards code must meet.

## Before you finish a change (required checks)

Every change must leave the workspace clean. Run these and **report the actual result** — do not
claim a change is done if you have not verified it:

```sh
cargo fmt
cargo check --workspace
cargo clippy --workspace
```

**Zero warnings is a merge gate** across the whole workspace. Treat clippy lints as house
style; never silence a lint without a comment saying why.

## Agent operating rules

- **You do not merge.** Leave `cargo check` and `cargo clippy` clean with zero warnings. If you
  cannot verify this, say so explicitly.
- **Match the surrounding code.** Read a neighbouring file first; follow its idioms, naming, and
  structure. Prefer extending an existing trait seam to inventing a new one.
- **Keep scope minimal.** Change only what the task needs. No unrelated refactors, no speculative
  abstractions "for later", no new dependencies without cause. Flag out-of-scope cleanup
  separately.
- **Never invent facts.** Verify a symbol, route, config key, or file path exists before relying
  on it. Cite real locations (`server/src/lib.rs:42`) when referencing code.
- **Ask before irreversible or outward-facing actions** — deleting files you didn't create,
  changing public interfaces or DB migrations, or touching a deployed system.

---

## Code style & design principles

### 1. Names reveal intent

Code is read far more often than it is written. A good name removes the need for a comment.

- Name things for **what they mean**, not what they are: `elapsed_days`, not `d`;
  `shortlisted_agents`, not `list1`.
- Follow Rust conventions: `snake_case` functions/variables/modules, `CamelCase` types/traits,
  `SCREAMING_SNAKE_CASE` constants. Types are nouns, functions are verbs.
- Name length matches scope: `i` is fine in a 3-line loop; anything module-level or public gets a
  full descriptive name.
- **One word per concept**, used everywhere: pick `fetch` *or* `get` *or* `load` — don't mix them
  for the same idea.
- A name must describe its side effects: a `get_*` that also creates something is misnamed
  (`get_or_create_*`).
- No magic values. Replace inline numbers/strings with named constants:
  `const MAX_TURNS: usize = 15;`.
- Avoid vague filler words (`Manager`, `Processor`, `Data`, `Info`, `Util` as a dumping ground) —
  they hide the real responsibility.

### 2. Functions: small, one thing, one level of abstraction

The highest-leverage rule in the repo, especially in async orchestration and request-handling
paths, where functions naturally balloon.

- A function does **one thing**. Test: if you can extract a block and give it a name that isn't
  just a restatement of its body, extract it.
- **One level of abstraction per body.** Don't mix high-level flow ("route, then call, then
  persist") with low-level detail (string fiddling, JSON assembly) — push details into named
  helpers and compose with `?`.
- Keep functions short — a screenful is the ceiling, not the target. Decompose long `async`
  flows into named stages, each returning `Result`.
- **Few arguments:** zero > one > two. Wrap 3+ related parameters in a struct.
- **No boolean flag arguments.** A flag means the function does two things — split it into two
  well-named functions.
- **No hidden side effects.** The name plus the signature (`&self` vs `&mut self`, ownership) must
  tell the whole story. A function that checks something must not also mutate state.
- Return values instead of out-parameters; reserve `&mut` for genuine in-place mutation.

### 3. Don't repeat yourself

Duplication is the single biggest maintainability tax: every copy is a bug fixed once and shipped
N−1 times broken.

- Logic that appears twice becomes a function; logic that appears in two crates becomes a shared
  crate or module.
- Near-duplicates count. If two blocks differ only in a value or a type, factor the common shape
  out (generics, a trait, or a parameter) and keep only the difference.
- The same applies to knowledge, not just code: a protocol type, a default value, or an endpoint
  format is defined in exactly one place and imported everywhere else.

### 4. Modules and crates: one reason to change

Size is measured in **responsibilities**, not lines.

- A module or crate owns one concern, nameable in a word or two. If you can't name it concisely,
  it does too much — split it into siblings.
- Prefer many small, well-named modules over a few large files. The compiler doesn't care;
  readers do.
- Keep public surface minimal: expose the least `pub` that works, keep helpers private. Small,
  tight interfaces mean low coupling.
- Order a file top-down like a newspaper: public/high-level items first, private helpers below.
- Dependencies flow one way: binaries depend on libraries, libraries depend on shared foundational
  crates, nothing depends back on a binary. Never `pub` an internal just so a sibling can reach in.
- Declare each external dependency once (workspace-level, in the root `Cargo.toml`) and reuse it
  via `dep.workspace = true`; one version of each dependency across the repo.

### 5. Depend on traits, construct in one place

This is the core pattern of the repo — how alternative implementations (Docker vs K8s, OSS vs
enterprise, real vs test) coexist without touching callers.

- Every replaceable or external concern sits behind a **trait we define** (`ContainerRuntime`,
  `AuthService`, `RoutingEngine`, `ObservabilityProvider`, …). Consumers hold
  `Arc<dyn Trait>` or a generic bound and program to the trait, never the concrete type.
- Concrete implementations are chosen in exactly one place — the composition root at startup.
  Handlers and business logic **receive** dependencies; they never construct them.
- Extension layers **wrap** the base implementation (hold it as `inner` and add behavior); they
  never copy-paste it to tweak it. If wrapping isn't possible, the base needs a seam — add it
  there, in the base implementation, rather than forking the logic into the extension.
- Wrap third-party crates behind our own interface at the edge. Don't let vendor types spread
  through the codebase; if the crate is swapped later, only the wrapper changes.
- Only add a seam when a second implementation is real or imminent — don't trait-ify something
  that will only ever have one impl.

### 6. Error handling: typed, propagated, never panicking on input

- **Library code returns typed errors** — a `thiserror` enum with variants named for what the
  *caller* needs to distinguish. Callers match on variants, not strings.
- **Binaries and glue** (CLI, `main`, one-shot workers) may use `anyhow` — nobody downstream
  matches on their errors.
- Propagate with `?`; keep the happy path unadorned. Add context at boundaries instead of
  logging-and-swallowing mid-stack.
- **`unwrap`/`expect` only in tests and startup wiring**, always as `expect("why this cannot
  fail")`. In any request, agent, or message-handling path, bad input becomes an `Err`, never a
  panic — one panic can take down a task serving many users.
- Model "absent" with `Option`, "recoverable failure" with `Result`. Don't paper over uncertainty
  with a default that hides the problem.
- **Degrade deliberately.** When an external dependency (LLM, embedding service, tracing backend)
  is down, the call site defines its fallback explicitly and records that a fallback happened
  (e.g. `fallback_used=true`). Silent degradation is a debugging nightmare.

### 7. Comments explain *why* — code explains *what*

- Before writing a comment, try to make it unnecessary: rename, extract a function, or introduce
  an explanatory variable.
- Good comments state **why**: a non-obvious decision, an invariant, a workaround, a protocol
  quirk. Bad comments restate the line below them.
- Write `///` doc comments on public items — especially trait definitions, which are our
  contracts. State intent and invariants, not the obvious.
- **No commented-out code, no change-log comments** — git remembers. Delete freely.
- `TODO`/`FIXME` must reference a tracking issue (e.g. `TODO BACKEND-16:`), or they rot.

### 8. Formatting and lints: the tools decide

- `cargo fmt` is the single source of truth for layout. No manual alignment, no style debates in
  review.
- **Zero warnings is a merge gate** (see [required checks](#before-you-finish-a-change-required-checks)).
- Configuration and tunables live at the top: read env/config once at startup (see
  `config/src/lib.rs`) and pass values down. No `std::env::var` or magic URLs sprinkled
  through business logic.

### 9. Tests: fast, independent, behavioral

- **F.I.R.S.T.** — Fast, Independent, Repeatable, Self-validating, Timely. Unit tests need no
  network, DB, or Docker; use the test doubles that every trait seam ships with. Real
  infrastructure is reserved for explicitly tagged integration tests (`#[ignore]` when they need
  Ollama, an LLM, or live services).
- Test **behavior through public interfaces**, not private internals, so tests survive refactors.
- One concept per test, with a name that states the scenario
  (`routing_falls_back_to_first_candidate_when_llm_fails`).
- Cover boundary conditions, and test exhaustively around any fixed bug — bugs congregate.
- Test code is held to the same standard as production code; a dirty test suite stops being run.

### 10. Concurrency: lean on the type system

We're async (`tokio`); ownership and `Send`/`Sync` prevent most data races at compile time — the
rules below cover what they can't.

- **Isolate concurrency.** Spawning, scheduling, and channel wiring live in a few well-named
  places; business logic stays task-agnostic and testable synchronously.
- **Minimize shared mutable state.** Prefer message passing and owned per-task data over shared
  locks; share read-only data as `Arc<T>`.
- **Never hold a lock or guard across an `.await`.** Keep critical sections tiny and synchronous.
- **Never block the runtime** — no blocking I/O or heavy CPU on an async worker; use
  `spawn_blocking`.
- Design shutdown in from the start: tasks exit cleanly when their channel closes; every
  background loop has a way to stop.

### 11. Clarity first, speed where measured

- **Don't clone to dodge the borrow checker.** Pass `&str`/`&[T]`/`&T` for read-only access; take
  ownership only when you store it. Reach for `.clone()` deliberately.
- Share large read-only data as `Arc<T>` — clone the pointer, not the payload. Hoist
  loop-invariant work out of loops.
- Let the type system do the checking for free: newtypes for IDs, enums for state machines — make
  invalid states unrepresentable instead of validating at runtime.
- **Measure before optimizing.** Profile a real hot path before trading readability for speed; no
  micro-optimizations on cold paths.

### 12. The habit: continuous small cleanup

Good design emerges from repeating four checks, in priority order:

1. **All tests pass.**
2. **No duplication.**
3. **Intent is expressed** — good names, small functions, standard patterns.
4. **Nothing extra** — no more modules, traits, or abstractions than the design needs.

And the **Boy Scout Rule**: leave every file a little cleaner than you found it. Constant small
improvement is how the repo stays clean without ever needing a rewrite.

---

## Review checklist

Every change — human-authored or agent-authored — must pass this before merge.

- [ ] Names reveal intent; no magic values; consistent vocabulary.
- [ ] Functions do one thing at one abstraction level; no flag args; 0–2 params.
- [ ] No duplicated logic or knowledge — shared code is factored out.
- [ ] Each module/crate has one responsibility; public surface is minimal.
- [ ] Dependencies are received via traits and wired at the composition root; vendor types stay behind wrappers; extensions wrap trait seams rather than forking logic.
- [ ] Library errors are typed; `?` everywhere; no `unwrap`/`expect` off the happy path; fallbacks are explicit and recorded.
- [ ] Comments say *why*; no commented-out code; public items have doc comments.
- [ ] `cargo fmt` clean; zero warnings from `check`/`clippy` across the whole workspace.
- [ ] Tests are fast, independent, behavioral; boundaries covered.
- [ ] No locks held across `.await`; runtime never blocked; shutdown is clean.
- [ ] Clones and allocations are deliberate; optimizations are measured.
- [ ] Scope is minimal — no unrelated refactors, speculative abstractions, or new dependencies snuck in.
