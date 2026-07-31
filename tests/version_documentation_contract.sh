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
assert_contains "$ROOT/docs/AGENT_MCP_CONTRACT.md" "\`v$version\` release"
assert_contains "$ROOT/docs/CLI_CONTRACT.md" "supported \`v$version\` contract"
assert_contains "$ROOT/docs/COMMAND_GRAMMAR.md" "supported \`v$version\` command surface"
assert_contains "$ROOT/docs/JSON_SCHEMAS.md" "supported v$version release"
assert_contains "$ROOT/docs/SHOWCASE_CONFORMANCE.md" "\`$version\` bounded release matrix"
assert_contains "$ROOT/tests/installer_contract.sh" "SUPPORTED_VERSION=\"$version\""
assert_contains "$ROOT/.github/workflows/ci.yml" "bounded v$version release scope"
assert_contains "$ROOT/.github/workflows/continuous-tui-eval.yml" "default: \"$version\""
assert_contains "$ROOT/.github/workflows/production-tui-canary.yml" "default: \"$version\""
assert_contains "$ROOT/agent-integrations/codex/heyfood/.codex-plugin/plugin.json" \
  "\"version\": \"$version\""
assert_contains "$ROOT/agent-integrations/claude/heyfood/.claude-plugin/plugin.json" \
  "\"version\": \"$version\""
assert_contains "$ROOT/crates/heyfood-bin/tests/installed_showcase.rs" \
  'format!("not enabled in the default {expected_version} artifact")'
assert_contains "$ROOT/crates/heyfood-bin/tests/installed_showcase.rs" \
  'format!("not a {expected_version} gate")'
assert_contains "$ROOT/crates/heyfood-bin/tests/installed_showcase.rs" \
  'format!("deferred from the supported {expected_version} contract")'
assert_contains "$ROOT/crates/heyfood-bin/tests/installed_showcase.rs" \
  '"production_human_presentation_journeys"'
presentation_gate_count=$(
  grep -Fc '"production_human_presentation_journeys"' \
    "$ROOT/crates/heyfood-bin/tests/installed_showcase.rs"
)
[[ "$presentation_gate_count" -eq 2 ]] ||
  fail "the human-presentation production gate must remain in both signed and unsigned evidence branches"
if grep -Fq '0.5.0' "$ROOT/crates/heyfood-bin/tests/installed_showcase.rs"; then
  fail "installed-artifact evidence must not retain stale v0.5.0 release copy"
fi
[[ "$(jq -r '.release' "$ROOT/tests/showcase/core-release-matrix.v1.json")" == "$version" ]] ||
  fail "the installed-artifact matrix must target v$version"

for source in "$ROOT/README.md" "$ROOT/docs/CAPABILITY_STATUS.md"; do
  if grep -Eq 'v0\.4\.[01]' "$source"; then
    fail "$source must not advertise obsolete incident releases"
  fi
done

printf 'version documentation contract: v%s coordinated\n' "$version"
