# Measured results

Everything here was produced by the commands in README.md on this machine (Apple M4, 16 GB,
macOS, `cargo --release`), October 2026, with the labelled splits in
`llm-router/tests/data/classifier/` and the public ten-case sample. Figures come from
`classifier_report`; the raw `OUT`/sidecar files are reproducible with the commands shown.
Laya figures are from the in-process Rust ONNX path, not from the Python package.

## Status at a glance

| backend | public sample (10) | own held-out (48) | verified? |
|---|---|---|---|
| regex (default) | 30.0 % acc, ECE 0.200 | 27.1 % acc, macro F1 0.258, ECE 0.138 | yes, measured here |
| laya (local, in-process ONNX) | 50.0 % acc, ECE 0.251 | 56.2 % acc, macro F1 0.542, ECE 0.159, 1/48 timeout fallback | yes, measured here |
| jev-1.13.0 (hosted) | **not run — credentials/billing unavailable** | **not run — credentials/billing unavailable** | no |

The Jev adapter is implemented and tested against a local HTTP mock of the documented
contract; no live Jev call was made. No hosted number is estimated anywhere in this file.

## Three-way comparison, same data, same scorer

| metric | regex | laya | jev |
|---|---|---|---|
| **held-out (48) request-type accuracy** | 27.1 % | **56.2 %** | not run |
| held-out macro F1 | 0.258 | 0.542 | not run |
| held-out ECE (10 equal-width bins) | 0.138 (fixed 0.5/0.3) | 0.159 | not run |
| held-out complexity exact / ±1 / MAE | 12.5 % / 43.8 % / 1.44 (fixed 3) | 33.3 % / 77.1 % / 0.98 | not run |
| held-out fallback rate (by reason) | 0 | 2.1 % (timeout = 1) | not run |
| held-out effective policy (fallback rows scored as regex) | — | 56.2 % over 48 | not run |
| held-out raw model only (fallback row excluded) | — | 57.4 % over 47 | not run |
| public sample (10) accuracy / macro F1 / ECE | 30.0 % / 0.262 / 0.200 | 50.0 % / 0.456 / 0.251 | not run |
| calibration (24) accuracy / ECE | 20.8 % / 0.133 | 37.5 % / 0.259 | not run |
| dev (48) accuracy / ECE | 35.4 % / — | 62.5 % / 0.131 | not run |
| decision latency p50 / p95 (held-out, excludes init) | 4 µs / 67 µs | 843 ms / 1 983 ms | not run |
| one-time initialization | ~12 ms (regex compile) | 1.1–5.2 s (bundle load; first load after a cold page cache was the 5.2 s) | n/a |
| model size / memory | — | 1.69 GB fp32 ONNX + 3.8 MB graph; 1.72–1.78 GB max RSS | n/a (hosted) |
| input per decision | — | 492 tokens (two questions, rubric included) | not run |
| cost per decision | $0 | **no API fee; ~0.85 s of M4 CPU per decision (≈4 CPU-seconds across threads)**, ~2 GB RAM resident | $0.042 / M input tokens (vendor page, Oct 2026), not measured |
| repeatability (two fresh runs, public sample, semantic fields) | 0/10 differ | 0/10 differ | not run |

"No API fee" is not zero cost: a Laya decision occupies a CPU core-equivalent for roughly a
second and keeps ~2 GB of memory resident for the life of the process.

### Laya, per class (held-out, 48)

| class | support | P | R | F1 |
|---|---|---|---|---|
| code_generation | 9 | 0.45 | 0.56 | 0.50 |
| code_understanding | 6 | 0.57 | 0.67 | 0.62 |
| technical_design | 6 | 0.50 | 0.83 | 0.62 |
| analytical_reasoning | 5 | 0.25 | 0.20 | 0.22 |
| writing | 8 | 1.00 | 0.25 | 0.40 |
| factual_lookup | 8 | 0.67 | 0.75 | 0.71 |
| general | 6 | 0.80 | 0.67 | 0.73 |

Confusion (rows truth, cols predicted; order code_gen, code_und, design, analysis, writing,
factual, general):

```
code_gen  5 1 2 1 0 0 0
code_und  0 4 0 2 0 0 0
design    0 0 5 0 0 0 1
analysis  0 2 1 1 0 1 0
writing   4 0 1 0 2 1 0
factual   2 0 0 0 0 6 0
general   0 0 1 0 0 1 4
```

The failures are systematic, not noise: **writing about technical topics is read as
code_generation** (4 of 8), and **analytical_reasoning is spread across code_understanding,
design and factual** (1 of 5 right). Both are the boundaries the rubric spells out; the base
checkpoint does not honour them reliably zero-shot. technical_design is over-predicted
(10 predicted for 6 true). Regex, by contrast, answers `general` for 37 of 48 held-out
cases, which is why its accuracy sits near the `general` prior.

### Confidence semantics and calibration

- Regex: fixed 0.5 (a pattern voted) / 0.3 (nothing matched). Uncalibrated by construction;
  on held-out the 0.3 bin is 16 % accurate and the 0.5 bin 64 %.
- Laya: probability of the chosen type after the checkpoint's own temperature
  (`choice:6-10` = 1.000). Held-out bins: [0.3,0.4) 30 % accurate, [0.4,0.5) 56 %,
  [0.5,0.6) 43 %, [0.6,0.7) 86 %, [0.7,0.8) 40 %, [0.8,0.9) 100 %, [0.9,1.0) 75 % — roughly
  monotone but noisy at n = 4–10 per bin; ECE 0.159. The model card's warning that base
  checkpoints "ship over-confident" is consistent with the 0.7–0.8 bin.
- Jev: would be the chosen-type probability as well (adapter contract); unmeasured.

### Selective accuracy and the confidence floor

Calibration split (24 cases) is what the floor must be chosen on; it is small, so this is a
reading, not a tuning:

| floor | calibration coverage / accuracy | held-out coverage / accuracy (reported after the fact) |
|---|---|---|
| 0.0 | 100 % / 37.5 % | 100 % / 56.2 % |
| 0.4 | 75 % / 38.9 % | 77 % / 64.9 % |
| 0.5 | 62.5 % / 46.7 % | 58 % / 67.9 % |
| 0.6 | 50 % / 50.0 % | 44 % / 76.2 % |

**Decision:** `CLASSIFIER_MIN_CONFIDENCE` stays `0.0` by default. A floor of 0.5 is the
smallest value where the calibration split shows a clear accuracy gain, and on held-out it
trades 42 % of cases (routed to the configured model instead) for +12 points on the rest.
Operators who prefer fewer, surer classifications can set 0.5; the held-out column above
was not used to pick it.

### Latency and the timeout

Laya on this CPU: p50 0.84 s, p95 2.0 s (held-out), 2.5 s on the second public run. The one
held-out fallback (`ho-016`, a 4-sentence design request) exceeded the default 3 s deadline;
the regex answer was served and the inference job finished on its own afterwards. For a
Laya deployment, `CLASSIFIER_TIMEOUT_MS=6000` is a better fit than the hosted-oriented
default; the default was left unchanged so regex/Jev behaviour is untouched. Threads: the
runs above used ONNX Runtime's default intra-op threads (`CLASSIFIER_THREADS=0`). A sweep on
the public sample (10 cases) on this 10-core M4 (4 performance + 6 efficiency cores):

| `CLASSIFIER_THREADS` | p50 | p95 | timeouts at 3 s |
|---|---|---|---|
| 0 (runtime default) | 1.25 s | 2.62 s | 0 |
| 4 | 0.96 s | 1.99 s | 0 |
| 10 | 2.72 s | 3.03 s | 5 of 10 |

Pinning threads to the performance-core count helps; using every core oversubscribes and
is far worse. Single runs, so treat the figures as indicative; the default stays `0`.

### Repeatability

Two consecutive release runs on the public sample: 0 of 10 rows differ on request type,
complexity or confidence for regex and for Laya (`latency_us` differs, as expected). Laya's
forward pass is deterministic on CPU; the Rust port also matches the PyTorch reference to
5e-5 (`tests/laya_local.rs`).

## Runtime feasibility (15-minute CPU runner)

- Regex: about one second for 10 cases, under two for 48.
- Laya: 48 cases took 49 s wall clock here including a 1.8 s load; a 200-case private set
  extrapolates to ~3–4 min on comparable hardware, ~8–10 min at half the per-core speed.
  Worst case with every call timing out at 3 s is 200 × 3.25 s ≈ 11 min, still inside the
  limit, each such row a counted regex fallback. Memory need is ~2 GB.
- Jev: network-bound and unmeasured; same worst-case bound.

## Exact commands

```sh
# regex
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/regex.jsonl cargo run --release -p nasiko-llm-router --example classifier_eval
# laya (after llm-router/scripts/laya-setup.sh)
CLASSIFIER_BACKEND=laya CLASSIFIER_MODEL_PATH=$PWD/.laya/model \
CLASSIFIER_ORT_DYLIB=$PWD/.laya/onnxruntime/lib/libonnxruntime.1.28.0.dylib \
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/laya.jsonl cargo run --release -p nasiko-llm-router --example classifier_eval
# jev (needs TYPESAFE_API_KEY; not run here)
CLASSIFIER_BACKEND=jev EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/jev.jsonl cargo run --release -p nasiko-llm-router --example classifier_eval
# score any of them (PRED2 adds the repeatability diff; PRICE_PER_M_INPUT adds the hosted cost line)
EVAL_SET=/tmp/classifier-eval.json PRED=/tmp/laya.jsonl cargo run --release -p nasiko-llm-router --example classifier_report
# parity, repeatability and timeout fallback on the real local model
CLASSIFIER_MODEL_PATH=… CLASSIFIER_ORT_DYLIB=… cargo test -p nasiko-llm-router --test laya_local -- --ignored --nocapture
```

Held-out, calibration and dev runs use `EVAL_SET=llm-router/tests/data/classifier/<split>.json`.

## Harness ambiguity to flag

The brief says runs are diffed for determinism, but `latency_us` is required to be a real
measurement and will differ between runs. This contribution keeps real timing and provides
`classifier_report … PRED2=…` to diff the semantic fields. Organizers should confirm whether
their diff excludes `latency_us`.

## Downstream routing quality

Not measured. No claim of reduced spend or better answers is made; see DECISIONS.md §4.
