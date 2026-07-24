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

case "$target" in
  aarch64-apple-darwin | x86_64-apple-darwin | aarch64-unknown-linux-gnu | x86_64-unknown-linux-gnu) ;;
  *)
    echo "unsupported smoke target: $target" >&2
    exit 64
    ;;
esac

archive="$release_directory/heyfood-v$version-$target.tar.gz"
staging=$(mktemp -d "${TMPDIR:-/tmp}/heyfood-smoke.XXXXXX")
trap 'rm -rf "$staging"' EXIT
tar -xzf "$archive" -C "$staging"
binary="$staging/heyfood"
test -f "$binary"
test -x "$binary"
if [[ "$target" == *-apple-darwin ]]; then
  if [[ -z "${HEYFOOD_APPLE_TEAM_ID:-}" ]]; then
    echo "expected Apple developer team is required for macOS smoke" >&2
    exit 78
  fi
  codesign --verify --deep --strict --verbose=2 "$binary"
  observed_team_id=$(
    codesign --display --verbose=4 "$binary" 2>&1 |
      sed -n 's/^TeamIdentifier=//p'
  )
  if [[ "$observed_team_id" != "$HEYFOOD_APPLE_TEAM_ID" ]]; then
    echo "installed macOS executable is not signed by the expected Apple developer team" >&2
    exit 78
  fi
  spctl --assess --type execute --verbose=2 "$binary"
fi
test "$("$binary" --version)" = "heyfood $version"
"$binary" --help >/dev/null
"$binary" register --help >/dev/null
"$binary" completion bash >"$staging/completion.bash"
test -s "$staging/completion.bash"
