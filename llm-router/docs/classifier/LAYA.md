# Laya backend (local, in-process)

Laya is Convai Innovations' open-weights "System 1" decision model: a ModernBERT-large encoder
(421M parameters) with a decision head that answers typed questions — a Choice over options,
a Score over an ordered scale — in one forward pass, returning calibrated probabilities. It is
the same question shape Jev answers, which is why both adapters share one rubric
(`routing/rubric.rs`) and are directly comparable. Apache-2.0, runs on CPU, needs no account.

## What was verified before writing the adapter (October 2026)

| Source | Checked |
|---|---|
| `pip install laya` 0.3.24 (Python, PyTorch 2.14.1, transformers 5.18) | pinned checkpoint `convaiinnovations/laya@55cf4c4e…` downloads (808 MB safetensors) and answers on CPU; `common.build_sequence` layout, `temp_bucket` + clamp `[0.5, 5]`, `answer_confidence = max(p)` vs entropy `confidence`, 512-token `max_len`, 192-token `head_max_len`, silent state truncation |
| `@receptron/laya` 0.1.2 (TypeScript, MIT) + `receptron/laya-onnx@68f27dfe…` (Apache-2.0) | ONNX export of the same checkpoint: inputs `input_ids`, `attention_mask`, `marker_pos`, `marker_mask` (bool), `qtype`; output `logits [B,K]`; `laya_config.json` carries the temperatures |
| Parity | ONNX bundle vs PyTorch on the public sample: max probability difference 5.0e-5, 10/10 labels agree (Python onnxruntime); the **Rust** port vs the PyTorch reference: max 5e-5, 10/10 labels (`tests/laya_local.rs`, fixture `tests/data/classifier/laya-reference-public.json`) |

The model card's own caveats apply: base checkpoints "ship over-confident" and underperform
zero-shot on typed decisions; English checkpoint only here (a multilingual variant exists).

## Setup (no Python, no key)

```sh
llm-router/scripts/laya-setup.sh            # → ./.laya (model 1.7 GB + onnxruntime 1.28.0)
export CLASSIFIER_BACKEND=laya
export CLASSIFIER_MODEL_PATH=$PWD/.laya/model
export CLASSIFIER_ORT_DYLIB=$PWD/.laya/onnxruntime/lib/libonnxruntime.1.28.0.dylib   # .so on Linux
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
cargo run --release -p nasiko-llm-router --example classifier_eval
```

The script pins the Hugging Face revision and verifies every file's sha256; it writes
`BUNDLE.json` (repo, revision, base model, license) which the adapter reports as the model
version. Weights are never committed (`.laya/` is gitignored).

| Setting | Meaning |
|---|---|
| `CLASSIFIER_MODEL_PATH` | directory with `laya.onnx`, `laya.onnx.data`, `laya_config.json`, `tokenizer/` |
| `CLASSIFIER_ORT_DYLIB` (or `ORT_DYLIB_PATH`) | ONNX Runtime shared library; `ort` loads it dynamically, nothing is downloaded at build time |
| `CLASSIFIER_THREADS` | intra-op threads (0 = runtime default) |
| `CLASSIFIER_TIMEOUT_MS`, `CLASSIFIER_MAX_CONCURRENCY`, `CLASSIFIER_MIN_CONFIDENCE` | as for every backend |

Resource requirements measured on this machine (Apple M4, 16 GB): load 4–5 s, ~2 GB RSS,
one decision (two questions in one batch) p50 ≈ 0.7 s on CPU. See RESULTS.md.

## How it works (`routing/laya.rs`)

- Sequence per question: `[CLS] <type> question: <instructions> [SEP] [MASK] opt0 [MASK] opt1 …
  [SEP] {"query": …, "context": …} [SEP]`; one logit per `[MASK]` marker. Two rows (type,
  complexity) are collated into one run.
- Budgets: 48 tokens per option, 192 for the whole head, the state gets the rest of 512 and is
  cut at the end. With this rubric about 200 tokens of query+context survive; the cut is
  reported as `input_truncated` per call (the service merges it with its own character caps).
  **This is the main behavioural difference from Jev (32k state budget).**
- Probabilities: per-cardinality temperature from `laya_config.json`, clamped `[0.5, 5]` like
  the Python package (`choice:6-10` = 1.000 and `score:3-5` = 1.251 are what this rubric uses),
  then softmax. Public confidence = probability of the chosen type (Laya's `answer_confidence`);
  entropy confidence kept as `vendor_*_confidence`. Complexity = mode of the Score
  distribution, ties low — identical rule to Jev.
- Concurrency: a semaphore permit is acquired inside the deadline *before* the job is queued on
  the blocking pool; the job owns the permit and an `Arc` to the session, so a timed-out caller
  falls back to regex immediately while the backlog stays bounded by `CLASSIFIER_MAX_CONCURRENCY`.
- Initialization happens only for `CLASSIFIER_BACKEND=laya`: regex and Jev deployments never
  open the bundle or load the runtime. A missing or corrupt bundle is an `init` fallback.

## Packaging obstacles, stated

- Two new workspace dependencies: `ort` 2.0.0-rc.13 (`load-dynamic`, `ndarray`, no binary
  download) and `tokenizers` 0.23 (`onig`, a bundled C build). They compile offline after
  `cargo fetch`; the ONNX Runtime *library* is a run-time artifact the setup script fetches.
- The ONNX bundle is fp32 (1.69 GB). A quantized export would be smaller and faster; none is
  published, and producing one was out of scope.
- Only the English checkpoint is wired. Multilingual input is accepted but degrades.
