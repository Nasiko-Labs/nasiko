#!/usr/bin/env bash
# Docker runner for machines without a local Rust toolchain. Secrets are inherited
# by environment-variable name, never embedded in command-line values or files.
set -euo pipefail
mode="${1:-off}"
case "$mode" in
  off|deterministic|jev|hybrid) ;;
  *) printf 'Usage: %s [off|deterministic|jev|hybrid]\n' "$0" >&2; exit 2 ;;
esac
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
eval_path="${EVAL_SET:-/tmp/compact-tools-eval.json}"
if [[ ! -f "$eval_path" ]]; then
  printf 'Set EVAL_SET to an existing evaluator dataset. See llm-router/README.md.\n' >&2
  exit 2
fi
eval_path="$(cd "$(dirname "$eval_path")" && pwd)/$(basename "$eval_path")"
result_path="${OUT:-$repo_dir/target/compact-tools-$mode.jsonl}"
mkdir -p "$(dirname "$result_path")"
result_dir="$(cd "$(dirname "$result_path")" && pwd)"
result_name="$(basename "$result_path")"
docker run --rm \
  -v "$repo_dir:/work" -v "$eval_path:/data/eval.json:ro" -v "$result_dir:/results" \
  -v nasiko-cargo-registry:/usr/local/cargo/registry \
  -v nasiko-cargo-git:/usr/local/cargo/git \
  -w /work -e "CARGO_NET_OFFLINE=${CARGO_NET_OFFLINE:-false}" \
  -e EVAL_SET=/data/eval.json -e "OUT=/results/$result_name" -e "EVAL_SELECTION_MODE=$mode" \
  -e TYPESAFE_API_KEY -e TYPESAFE_BASE_URL -e TYPESAFE_MODEL \
  -e TOOL_SELECTION_THRESHOLD -e TOOL_SELECTION_UNCERTAINTY_FLOOR \
  -e TOOL_SELECTION_MAX_TOOLS -e TOOL_SELECTION_MAX_TOKENS -e TOOL_SELECTION_MIN_TOOLS \
  -e TOOL_SELECTION_MIN_CATALOG_TOKENS -e TOOL_SELECTION_TIMEOUT_MS -e TOOL_SELECTION_JEV_MODEL \
  -e TOOL_SELECTION_FALLBACK -e TOOL_SELECTION_MANDATORY -e TOOL_SELECTION_DEPENDENCIES \
  -e PROVIDER_BASE_URL -e PROVIDER_API_KEY -e MODEL -e REQUEST_TIMEOUT_SECS \
  "${NASIKO_RUST_IMAGE:-rust:latest}" \
  cargo run --release --locked -q -p nasiko-llm-router --example compact_tools_eval
printf 'Evaluation JSONL: %s/%s\n' "$result_dir" "$result_name"
