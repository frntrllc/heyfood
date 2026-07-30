#!/usr/bin/env bash

set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT

fail() {
  printf 'version documentation contract: %s\n' "$*" >&2
  exit 1
}

assert_contains() {
  local source=$1
  local expected=$2
  grep -Fq -- "$expected" "$source" ||
    fail "$source must contain: $expected"
}

version=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$ROOT/Cargo.toml" | head -1)
readonly version
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  fail "Cargo.toml must declare an exact workspace version"

assert_contains "$ROOT/install.sh" "SUPPORTED_VERSION=\"$version\""
assert_contains "$ROOT/README.md" "supported release is \`$version\`"
assert_contains "$ROOT/README.md" "supported native \`v$version\` binary"
assert_contains "$ROOT/docs/CAPABILITY_STATUS.md" \
  "| Native v$version | Current supported release |"
assert_contains "$ROOT/docs/CAPABILITY_STATUS.md" \
  "| Current native release | v$version |"
assert_contains "$ROOT/docs/CAPABILITY_STATUS.md" \
  "checksum-verified native \`v$version\` archive"
assert_contains "$ROOT/docs/RELEASE_SIGNING.md" "\`v$version\` tag-driven release"
assert_contains "$ROOT/CHANGELOG.md" "## $version -"

for source in "$ROOT/README.md" "$ROOT/docs/CAPABILITY_STATUS.md"; do
  if grep -Eq 'v0\.4\.[01]' "$source"; then
    fail "$source must not advertise obsolete incident releases"
  fi
done

printf 'version documentation contract: v%s coordinated\n' "$version"
