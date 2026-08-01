#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 || $# -gt 4 ]]; then
  echo "usage: smoke.sh RELEASE_DIRECTORY VERSION TARGET [--native-state]" >&2
  exit 64
fi

release_directory=$1
version=$2
target=$3
native_state_mode=${4:-}
if [[ -n "$native_state_mode" && "$native_state_mode" != "--native-state" ]]; then
  echo "unsupported smoke mode: $native_state_mode" >&2
  exit 64
fi
if [[ "$native_state_mode" == "--native-state" ]]; then
  "$(dirname "$0")/verify-assets.sh" \
    "$release_directory" \
    "$version" \
    --native-state
  "$(dirname "$0")/smoke-archive.sh" \
    "$release_directory" \
    "$version" \
    "$target" \
    --complete-set-verified \
    --native-state
else
  "$(dirname "$0")/verify-assets.sh" "$release_directory" "$version"
  "$(dirname "$0")/smoke-archive.sh" \
    "$release_directory" \
    "$version" \
    "$target" \
    --complete-set-verified
fi
