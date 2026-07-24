#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: smoke.sh RELEASE_DIRECTORY VERSION TARGET" >&2
  exit 64
fi

release_directory=$1
version=$2
target=$3
"$(dirname "$0")/verify-assets.sh" "$release_directory" "$version"
"$(dirname "$0")/smoke-archive.sh" \
  "$release_directory" "$version" "$target" --complete-set-verified
