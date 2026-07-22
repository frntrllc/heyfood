#!/usr/bin/env bash

set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT
readonly INSTALLER="$ROOT/install.sh"
readonly CHECKSUM="$ROOT/install.sh.sha256"
CASE_DIR=$(mktemp -d)
readonly CASE_DIR

cleanup() {
  rm -rf -- "$CASE_DIR"
}
trap cleanup EXIT

fail() {
  printf 'installer contract: %s\n' "$*" >&2
  exit 1
}

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$ROOT" && sha256sum -c "$(basename "$CHECKSUM")") ||
    fail "install.sh.sha256 does not match install.sh"
else
  (cd "$ROOT" && shasum -a 256 -c "$(basename "$CHECKSUM")") ||
    fail "install.sh.sha256 does not match install.sh"
fi

stdout="$CASE_DIR/stdout"
stderr="$CASE_DIR/stderr"
marker="$CASE_DIR/network-must-not-run"

set +e
HEYFOOD_VERSION=9.9.9 \
HEYFOOD_BIN_DIR="$CASE_DIR/bin" \
CURL="$marker" \
bash "$INSTALLER" >"$stdout" 2>"$stderr"
result=$?
set -e

[[ "$result" -eq 1 ]] || fail "suspended installer must exit 1"
[[ ! -s "$stdout" ]] || fail "suspended installer must keep stdout empty"
[[ ! -e "$marker" ]] || fail "suspended installer must not invoke the network"
[[ ! -e "$CASE_DIR/bin" ]] || fail "suspended installer must not create an install directory"
grep -Fq "installation is suspended" "$stderr" ||
  fail "suspended installer must explain the release state"
grep -Fq "do not install or use" "$stderr" ||
  fail "suspended installer must give an explicit safety instruction"

printf 'installer contract: suspended and fail-closed\n'
