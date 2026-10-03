# Strands type classifier and local complexity

`REQUEST_CLASSIFIER=strands` uses the pinned Strands Decider for the seven-way
request type and the retrained embedded model for complexity. It asks one remote
question instead of paying for two decoder passes. This is a composed backend;
its complexity results must not be attributed to the native Strands score head.
`strands_raw` retains the original two-question backend for comparisons.

The complete seven-option distribution is checked for valid labels, normalization,
and agreement with the selected label. Returned type confidence is the selected
label's probability. Native Strands confidence, `(7 * p_max - 1) / 6`, measures
concentration and is retained only by the raw baseline. Complexity confidence
comes from the local head's normalized entropy and weights the tier prior; it is
not a calibrated probability that the rounded complexity is correct.

The unchanged 0.4 floor sends uncertain requests to the configured model without
caching a routing decision. Optimized Strands additionally preserves the original
admission minimum as a selected-label probability of `(1 + 6 * 0.4) / 7`
(approximately 0.485714). Reporting probability confidence must not silently
weaken the old concentration-based gate. Literal confidence below 0.4 and
requests rejected by the full admission policy are reported separately. Transport,
timeout, malformed response and model-load
failures use regex. Regex remains the default. `hybrid` is an optional local-first
cascade with a fixed 0.70 local type threshold, and also uses regex on remote
failure; its quality must be measured separately before selecting it.

## Reproduction on Apple silicon

The serial MLX server is an evaluation runner, not a portable production service.
It binds to loopback, loads a pinned checkpoint and base revision, merges LoRA,
and optionally quantizes to 4-bit/group-64. It needs the pinned upstream
`strands_decider` package and its MLX/PyTorch dependencies in the Python environment.
The evaluated upstream source revision is
`890947e7ccd44c3de4115e26a7f46cc5c3147b44`. Model revisions are
recorded in `/health` and evaluation manifests.

```sh
# From the repository root, after installing the upstream MLX requirements:
python llm-router/eval/strands/serve_local.py --port 18099 --quantize 4 \
  --cache-limit-mib 128 --idle-warmup-seconds 5 \
  --warmup-body llm-router/eval/strands/calibration/native-warmup-body.json

# Wait for /ready before sending traffic. Startup is reported separately.
REQUEST_CLASSIFIER=strands STRANDS_URL=http://127.0.0.1:18099 \
  STRANDS_TIMEOUT_MS=1500 ROUTING_SEED=42 \
  EVAL_SET=llm-router/eval/h4-validation.json OUT=/tmp/strands.jsonl \
  cargo run --release -p nasiko-llm-router --example classifier_eval
```

`STRANDS_API_KEY` configures a bearer token for an authenticated endpoint or proxy.
The bundled loopback evaluation server does not implement token authentication.
`ROUTING_SEED` reproduces tier sampling for identical classifications and learned
cells under the same binary. Leaving it unset preserves normal exploration;
cached decisions remain sticky. The evaluation example returns classifications,
not downstream model answers or actual routing cost.

Readiness requires successful short and long inference warm-ups using the actual
client question rubric. The supplied native warm-up body is exported from the
same Rust helper with `--native-complexity`; it warms both one-question and
two-question clients before exposing the server. Startup warm-up occurs before accepting requests.
`X-Request-Deadline-Unix-Ms` lets the serial server skip expired queued requests
and reject results that exceed the deadline. A client timeout cannot cancel an
already executing Metal forward. This does not establish latency under concurrent
traffic, and shared-host memory pressure can still cause timeouts. No response
cache is enabled; repeated classifications perform actual inference.

## Memory protocol and idle recovery

The installed MLX allocator starts with a zero wired limit. The upstream
`mlx-lm` server and generation context opt into GPU residency, while this custom
decoder server originally omitted that step. On the shared 8 GB Mac, short
warm-up success followed by Laya execution still produced Strands timeouts;
the first 1.5-second WildChat pass exhausted the circuit breaker. Paging is a
possible explanation, not an established diagnosis. Bounded residency alone
also failed to prevent the initial pass from timing out. That failed
run remains part of the operational evidence.

The evaluation server now evaluates the final decoder after optional quantization,
clears temporary load buffers, and sets a process-local residency budget before
warming inference. Automatic residency is the smaller of the device's reported
recommended budget, physical memory minus 1 MiB, and final active MLX memory plus
the configured buffer-cache limit plus 512 MiB of inference headroom. `/health`
records the active bytes, selected wired budget and device limits. The previous
process limit is restored when the server exits or startup fails. Missing device
limits leave automatic residency disabled and are reported explicitly.

`--wired-limit-mib N` sets an explicit bounded residency budget; `0` disables it
for controlled comparisons. Values above the safe device budget fail startup.
The server changes only its MLX process setting, and does not change system GPU
limits or other processes. The effect on idle recovery and request latency must
be measured on the actual runner; bounded residency alone is not a concurrency
or latency guarantee. Keep failed and successful operational runs in separate
output directories, preserving load/warm-up times and server health metadata.

`--idle-warmup-seconds 5` enables an actual short inference after five idle
seconds. It is disabled by default (`0`). Maintenance shares the HTTP server's
single inference thread and uses the exact client rubric; it does not cache
responses. Normal POST attempts suppress unnecessary warm-ups, and health GETs
do not extend the inference activity timer. Health records the maintenance count,
latency and failures; a failed maintenance inference makes `/ready` return 503
until real or maintenance inference succeeds. This consumes compute while idle
and may briefly queue a new request, so it is an explicit deployment tradeoff.

Use this flag when reproducing the maintained operational comparison. Startup
warm-up and bounded residency alone both failed the first pass on the shared
host (`optimized-results` and `resident-results`, regenerated by `real-world/run.py`).
Idle maintenance is evaluated separately in `maintained-results`. These results
do not establish an SLA on concurrent or independent production traffic.

## Training-only confidence audit

`calibrate.py` selects 40 requests per type from the cleaned train-a through train-e
files, with one per declared family. Near-duplicates are grouped before five-fold
cross-validation. The Rust `strands_calibration_rows` example exports the exact
serving rubric. No H4, public-sample or WildChat labels are read when fitting.

```sh
python3 llm-router/eval/strands/calibrate.py prepare
cargo run --release -p nasiko-llm-router --example strands_calibration_rows -- \
  llm-router/eval/strands/calibration/selected-training.jsonl \
  > llm-router/eval/strands/calibration/requests.jsonl
python3 llm-router/eval/strands/calibrate.py collect --url http://127.0.0.1:18099
python3 llm-router/eval/strands/calibrate.py fit
```

The 280-row grouped-CV candidate improves NLL from 0.414 to 0.318 and ECE from
0.146 to 0.021, but increases accepted wrong decisions from 17 to 23 at the
probability-equivalent minimum of the original 0.4 concentration floor. The
acceptance gate requires improved NLL **and no increase in accepted wrong
decisions** at that minimum. The candidate is rejected; the shipped
temperature is 1.0 (identity), rather than sharpening confidence to reduce
abstention. The asset records the rejected fit and provenance. Probability
semantics alone do not imply calibration on an unseen domain.

## Verification

```sh
python3 -m unittest discover -s llm-router/eval/strands -p 'test_*.py'
cargo test --release -p nasiko-llm-router --lib
STRANDS_URL=http://127.0.0.1:18099 STRANDS_TIMEOUT_MS=30000 \
  cargo test --release -p nasiko-llm-router --test classifier_benchmark -- --nocapture
```

The long benchmark timeout separates classification quality from deadline
failures. For the operational 1.5-second comparison on the frozen real-user
sample, see [WildChat evaluation](../real-world/README.md). Independent human
labels, concurrent isolated deployment tests and downstream quality/cost
measurement remain necessary before claiming production routing savings.
