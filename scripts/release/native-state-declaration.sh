#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: native-state-declaration.sh VERSION" >&2
  exit 64
fi

version=$1
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
(
  cd "$root"
  cargo run --quiet --locked --package heyfood-installer -- \
    native-state-declaration "$version"
)
