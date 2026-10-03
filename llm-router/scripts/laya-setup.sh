#!/usr/bin/env bash
# Fetch the Laya ONNX bundle and an ONNX Runtime shared library for the local classifier backend.
#
#   llm-router/scripts/laya-setup.sh [DEST]        (default: ./.laya)
#
# Downloads, verifies (sha256) and lays out:
#   DEST/model/{laya.onnx,laya.onnx.data,laya_config.json,tokenizer/…,BUNDLE.json}
#   DEST/onnxruntime/lib/libonnxruntime.{dylib,so}
# then prints the env vars to export. Nothing is written outside DEST; nothing is committed.
#
# Provenance (pinned):
#   model   receptron/laya-onnx @ 68f27dfe5a27a54fb2b1fefc432f43f972e90868  (Apache-2.0)
#           = ONNX export of convaiinnovations/laya @ 55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851
#             (ModernBERT-large encoder + decision head, 421M params, fp32, English checkpoint)
#   runtime microsoft/onnxruntime v1.28.0 release archives (MIT)
# Sizes: laya.onnx.data 1.69 GB, laya.onnx 3.8 MB, tokenizer 3.6 MB, onnxruntime 10–42 MB.
# Needs ~2 GB of disk and ~2 GB of RAM at run time. No Python, no PyTorch, no account.
set -euo pipefail

DEST="${1:-./.laya}"
MODEL_REPO="receptron/laya-onnx"
MODEL_REV="68f27dfe5a27a54fb2b1fefc432f43f972e90868"
BASE_MODEL="convaiinnovations/laya@55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851"
ORT_VERSION="1.28.0"

FILES="laya.onnx laya.onnx.data laya_config.json tokenizer/tokenizer.json tokenizer/tokenizer_config.json"
expected_sha() {
  case "$1" in
    laya.onnx) echo a874eb254b58b0fcb1e7ad56fbb188c29d64e08c9a46b689433e1f52c66dba1e ;;
    laya.onnx.data) echo 487746363a8da57bcadb4345352997d22a0fb90d70aa22c6856668d023242aba ;;
    laya_config.json) echo 5049005dc6ae3ca5e82cc7d85c421357d5c543817300c8e8c5281ddbc69bb561 ;;
    tokenizer/tokenizer.json) echo 6c8aaa9a542084f2457eab775d4eeb51f92a70c0fd9de28d5edb0ddec3c08d30 ;;
    tokenizer/tokenizer_config.json) echo 50044de60daaa73df97d262e15a40d4faf0160e7d742df64b377877a1320dd12 ;;
  esac
}
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

MODEL_DIR="${DEST:?}/model"
mkdir -p "$MODEL_DIR/tokenizer"
for f in $FILES; do
  out="$MODEL_DIR/$f"
  want="$(expected_sha "$f")"
  if [ -f "$out" ] && [ "$(sha256 "$out")" = "$want" ]; then
    echo "ok       $f"
    continue
  fi
  echo "fetching $f"
  curl -fL --retry 3 -o "$out" "https://huggingface.co/$MODEL_REPO/resolve/$MODEL_REV/$f"
  got="$(sha256 "$out")"
  if [ "$got" != "$want" ]; then
    echo "checksum mismatch for $f: $got" >&2
    exit 1
  fi
done
cat > "$MODEL_DIR/BUNDLE.json" <<JSON
{"repo": "$MODEL_REPO", "revision": "$MODEL_REV", "base_model": "$BASE_MODEL", "license": "Apache-2.0", "onnxruntime": "$ORT_VERSION"}
JSON

# ONNX Runtime shared library for this platform (the Rust side loads it dynamically).
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  ORT_ASSET="onnxruntime-osx-arm64-$ORT_VERSION";  ORT_LIB="libonnxruntime.$ORT_VERSION.dylib" ;;
  Darwin-x86_64) ORT_ASSET="onnxruntime-osx-x86_64-$ORT_VERSION"; ORT_LIB="libonnxruntime.$ORT_VERSION.dylib" ;;
  Linux-x86_64)  ORT_ASSET="onnxruntime-linux-x64-$ORT_VERSION";  ORT_LIB="libonnxruntime.so.$ORT_VERSION" ;;
  Linux-aarch64) ORT_ASSET="onnxruntime-linux-aarch64-$ORT_VERSION"; ORT_LIB="libonnxruntime.so.$ORT_VERSION" ;;
  *) echo "unsupported platform $(uname -s)-$(uname -m); download onnxruntime $ORT_VERSION by hand" >&2; exit 1 ;;
esac
ORT_DIR="${DEST:?}/onnxruntime"
ARCHIVE="${DEST:?}/${ORT_ASSET:?}.tgz"
if [ ! -f "$ORT_DIR/lib/$ORT_LIB" ]; then
  echo "fetching $ORT_ASSET"
  mkdir -p "$ORT_DIR"
  curl -fL --retry 3 -o "$ARCHIVE" "https://github.com/microsoft/onnxruntime/releases/download/v$ORT_VERSION/$ORT_ASSET.tgz"
  tar -xzf "$ARCHIVE" -C "$ORT_DIR" --strip-components=1
  rm -f -- "${ARCHIVE:?}"
fi
echo
echo "Laya bundle ready. Export for the router, the evaluator and the tests:"
echo "  export CLASSIFIER_BACKEND=laya"
echo "  export CLASSIFIER_MODEL_PATH=$(cd "$MODEL_DIR" && pwd)"
echo "  export CLASSIFIER_ORT_DYLIB=$(cd "$ORT_DIR/lib" && pwd)/$ORT_LIB"
