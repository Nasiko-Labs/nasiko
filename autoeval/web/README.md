# AutoEval Live — local browser UI

Copy this `web` folder into your existing repository at `nasiko/autoeval/web/`.
The existing evaluator, Nasiko server and frozen benchmark are unchanged.

From the Nasiko repository root, run one command:

```sh
python3 autoeval/web/server.py
```

Open **http://127.0.0.1:8081**. Keep this terminal and Docker Desktop running;
Nasiko should already be reachable at http://localhost:8080.

1. Select **Calculator / Python** (or a real benchmark task).
2. Click **Detect** to inspect the toolchain and show production/test separation.
3. Check the two `provider:model` inputs, then click **Run live comparison**.
4. Watch the real CLI progress and logs. This makes fresh paid requests through
   Nasiko, not a playback. There are preflight requests followed by code generation.
5. Inspect the new response ID, Nasiko trace, tokens, generation duration, cost,
   and generated file diff. Scores come from the existing evaluator.
6. After completion, check the model-switch quality policy if desired.

The synthetic calculators remain labelled as synthetic even though **Run live
comparison** now asks real models to fix them. Use the real repository results
for the hackathon's measured real-work requirement. Real-task source is restored
from bundled history when available. A custom input accepts an existing local
Git checkout, bug-fix commit and issue; the core evaluator's limitations apply.

The UI shows the real coarse-grained stage, not invented percentage progress.
Detailed generation/grading activity remains in the live log and saved evidence.
Provider errors remain visible. It does not promise that a model succeeds.

New outputs go under `autoeval/runs/web-*`. No existing `final/` or `evidence/`
files are overwritten. The UI does not add code under `src/nasiko_autoeval`, so
the saved benchmark's source checksums are preserved.

## Model-switch gate

The UI evaluates a **quality policy**, not a deployed-model mutation. The candidate
must preserve every task the baseline resolved. Missing/infrastructure results,
different task sets, duplicate slots, or no baseline successes are INCONCLUSIVE.
An equal aggregate score does not hide a regression on an individual task.

For a pipeline to enforce it, run:

```sh
python3 autoeval/web/server.py --gate PATH/TO/results.json \
  --baseline gemini-3.5-flash-lite --candidate gemini-3.1-flash-lite
```

Exit codes are **0 PASS**, **1 BLOCK**, **2 INCONCLUSIVE**. Make a deployment or
model-switch job depend on this command succeeding. The command does not itself
change Nasiko configuration or deploy an application. A UI badge alone is not
deployment enforcement. Only claim the bonus as a pipeline gate once you have
wired and demonstrated that dependency.

Run the gate's focused tests:

```sh
python3 -m unittest discover -s autoeval/web -p test_server.py -v
```

## Local demo scope

Standard-library Python, no Node installation, frontend build or external CDN.
Bound to 127.0.0.1; Host/Origin checks and JSON-only POSTs prevent cross-origin
browser actions. Do not expose this local operator UI publicly. Existing login
credentials stay in the Python process and evaluator subprocess. The browser
never needs the `.env` file or provider keys. One comparison runs at a time.
Ctrl+C interrupts a running comparison and shuts down the UI.

The core grading limitations still apply, including lack of tamper-proof
framework execution. Saved ledger values are not provider invoice reconciliation.

Optional server arguments: `--port 8081`, `--server-url http://localhost:8080`,
`--postgres-container nasiko-postgres-1` (override for your actual container).
