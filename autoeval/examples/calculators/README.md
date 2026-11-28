# Synthetic calculator demos

These five small projects verify adapters and the shared evaluator. They are
synthetic, use no model requests, and are excluded from real benchmark scores.

| Folder | Toolchain | Command from Nasiko root |
| --- | --- | --- |
| `python/` | Python / pytest | `python3 autoeval/run.py examples python` |
| `javascript/` | JavaScript / Jest | `python3 autoeval/run.py examples javascript` |
| `typescript/` | TypeScript / Vitest | `python3 autoeval/run.py examples typescript` |
| `go/` | Go / go test | `python3 autoeval/run.py examples go` |
| `rust/` | Rust / Cargo | `python3 autoeval/run.py examples rust` |

All five in one command:

```sh
python3 autoeval/run.py examples all
```

Requirements: Python 3.11+, Git and running Docker. The helper prints the detected
framework and creates each isolated repository under `autoeval/.cache/calculators/`.
The language/framework is discovered from its actual project configuration.
No per-language evaluation orchestration exists in the helper.

Every visible source starts with the same bug: `add(a, b)` subtracts instead of
adding. The zero-input test passes on old code; positive/negative tests fail.
Each folder has:

- Its project manifest and ordinary source/test files.
- `history.json`: files and test path needed to reconstruct the history.
- `base-tests.txt`: the original zero-input control.
- `reference.patch`: the one-line production fix.

The generic helper creates deterministic `buggy` and `reference-fix` commits.
The first contains buggy production and the zero-input control. The second
contains the production fix and adds the two regression tests. Reference assets
and history-building metadata are excluded from both task trees. The ordinary
AutoEval task builder splits that second commit, then the shared Docker runner
checks old+regression and fixed+regression twice each.

Expected per example: **2 fail-to-pass and 1 pass-to-pass**. Output is labelled
`local_fixture` / `synthetic_calculator_demo`, under a fresh `synthetic-runs/`
directory. `--run NEW_DIRECTORY` selects another destination. Preserved actual
checks are at [evidence/calculators](../../evidence/calculators/report.md).

To inspect the prepared history, use the repository path printed by the helper:

```sh
git -C PATH_PRINTED_BY_THE_HELPER log --oneline
git -C PATH_PRINTED_BY_THE_HELPER show reference-fix
python3 autoeval/run.py inspect --repo PATH_PRINTED_BY_THE_HELPER
```

Rust supplies only a runtime-image override to reuse the cached slim image;
its Cargo toolchain and test classification are still detected automatically.
