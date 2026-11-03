#!/usr/bin/env bash
# Nasiko CLI installer: published to the registry as nasiko/install and served at
# https://registry.nasiko.dev/r/nasiko/install
#
# Usage:
#   curl -fsSL https://registry.nasiko.dev/r/nasiko/install | bash
#
# Environment overrides (see docs/CLI_INSTALL.md):
#   NASIKO_VERSION      version tag to install (default: latest)
#   NASIKO_INSTALL_DIR  install destination (default: ~/.local/bin)
#   NASIKO_REGISTRY     registry host (default: registry.nasiko.dev)
#
# Design constraints (do not "fix" these):
#   - Never invokes sudo; writes only to mktemp and the user-owned install dir.
#   - Never edits shell rc files; prints the PATH line instead.
#   - Anonymous pulls only: the nasiko/nasiko repository is public-read.
set -euo pipefail

REGISTRY="${NASIKO_REGISTRY:-registry.nasiko.dev}"
REPO="nasiko/nasiko"
VERSION="${NASIKO_VERSION:-latest}"
API="https://${REGISTRY}/v2/${REPO}"

fail() {
  echo "install: error: $*" >&2
  exit 1
}

command -v curl >/dev/null || fail "curl is required"

# --- 1. detect platform -------------------------------------------------------
case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=darwin ;;
  MINGW* | MSYS* | CYGWIN*)
    fail "Windows detected: use the PowerShell installer: irm https://${REGISTRY}/r/nasiko/install-ps1 | iex"
    ;;
  *) fail "unsupported operating system: $(uname -s)" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=amd64 ;;
  aarch64 | arm64) arch=arm64 ;;
  *) fail "unsupported architecture: $(uname -m)" ;;
esac
echo "→ installing nasiko ${VERSION} for ${os}/${arch}"

# --- helpers -------------------------------------------------------------------
# JSON extraction with jq when available, python3 otherwise. $1 is a jq
# expression, $2 the equivalent python expression over the parsed document `d`.
json_query() {
  if command -v jq >/dev/null; then
    jq -r "$1"
  elif command -v python3 >/dev/null; then
    python3 -c "import json,sys; d=json.load(sys.stdin); print($2)"
  else
    fail "either jq or python3 is required"
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    fail "either sha256sum or shasum is required"
  fi
}

# --- 2. resolve the multi-platform index to this platform's manifest digest ----
index=$(curl -fsSL \
  -H "Accept: application/vnd.oci.image.index.v1+json" \
  "${API}/manifests/${VERSION}") ||
  fail "cannot fetch ${REPO}:${VERSION} from ${REGISTRY}"
child=$(echo "$index" | json_query \
  "[.manifests[] | select(.platform.os==\"${os}\" and .platform.architecture==\"${arch}\") | .digest][0] // empty" \
  "next((m['digest'] for m in d.get('manifests',[]) if m.get('platform',{}).get('os')=='${os}' and m['platform'].get('architecture')=='${arch}'), '')")
[ -n "$child" ] || fail "release ${VERSION} has no ${os}/${arch} build"

# --- 3. resolve the child manifest to its single binary layer ------------------
manifest=$(curl -fsSL \
  -H "Accept: application/vnd.oci.image.manifest.v1+json" \
  "${API}/manifests/${child}") ||
  fail "cannot fetch manifest ${child}"
layer=$(echo "$manifest" | json_query \
  '.layers[0].digest // empty' \
  "(d.get('layers') or [{}])[0].get('digest','')")
[ -n "$layer" ] || fail "manifest ${child} has no layers"

# --- 4. download and verify against the content-addressed digest ---------------
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
echo "→ downloading ${layer}"
curl -fsSL -o "$tmp" "${API}/blobs/${layer}" || fail "blob download failed"
got="sha256:$(sha256_of "$tmp")"
[ "$got" = "$layer" ] || fail "digest mismatch: expected ${layer}, got ${got}"

# --- 5. install (user-owned prefix, never sudo) ---------------------------------
dir="${NASIKO_INSTALL_DIR:-${HOME}/.local/bin}"
mkdir -p "$dir" 2>/dev/null ||
  fail "cannot create ${dir}: set NASIKO_INSTALL_DIR to a writable directory"
[ -w "$dir" ] ||
  fail "${dir} is not writable: set NASIKO_INSTALL_DIR to a writable directory"
chmod 755 "$tmp" # mktemp makes it 0600
mv "$tmp" "${dir}/nasiko"
trap - EXIT

# --- 6. verify and hand off ------------------------------------------------------
echo "✓ installed $("${dir}/nasiko" --version) to ${dir}/nasiko"
case ":${PATH}:" in
  *":${dir}:"*)
    # On the PATH, but another nasiko earlier on it (a cargo install, say) would still win.
    found="$(command -v nasiko || true)"
    if [ -n "$found" ] && [ "$found" != "${dir}/nasiko" ]; then
      echo "⚠ another nasiko comes first on your PATH: ${found}"
      echo "    remove it, or put ${dir} before it in your shell profile"
    fi
    ;;
  *)
    echo "⚠ ${dir} is not on your PATH: add it to your shell profile:"
    echo "    export PATH=\"${dir}:\$PATH\""
    ;;
esac
