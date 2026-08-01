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
assert_contains "$ROOT/CHANGELOG.md" "/household add"
assert_contains "$ROOT/README.md" "local encrypted household roster"
assert_contains "$ROOT/README.md" "four native product archives and four"
assert_contains "$ROOT/README.md" \
  "Member/Everyone hosted guidance and evaluation fail locally"
assert_contains "$ROOT/README.md" \
  "installer and binary do not enforce the later native-state floor"
assert_contains "$ROOT/docs/CAPABILITY_STATUS.md" \
  "Supported in v$version: the human TUI adds or onboards active members atomically"
assert_contains "$ROOT/docs/CLI_CONTRACT.md" \
  "Persistent Me/member/Everyone scope selection"
assert_contains "$ROOT/docs/COMMAND_GRAMMAR.md" "/household add"
assert_contains "$ROOT/docs/HOUSEHOLD_LOCAL_STATE.md" \
  "part of the supported v$version TUI contract"
assert_contains "$ROOT/docs/HOUSEHOLD_LOCAL_STATE.md" \
  "cross-device roster sync"
assert_contains "$ROOT/docs/RELEASE_SIGNING.md" \
  "four \`heyfood\` product archives"
assert_contains "$ROOT/docs/RELEASE_SIGNING.md" \
  "The tag workflow does not rebuild, re-sign, repackage, or regenerate candidate"
assert_contains "$ROOT/docs/RELEASE_SIGNING.md" \
  "HEYFOOD_APPROVED_CANDIDATE_SHA256SUMS_SHA256"
assert_contains "$ROOT/docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md" \
  "Journey A — clean v$version install and household lifecycle"
assert_contains "$ROOT/docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md" \
  "Journey B — v0.6.2 to v$version upgrade and current-installer refusal"
assert_contains "$ROOT/docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md" \
  "Journey C — authorization rollover without household rebinding"
assert_contains "$ROOT/docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md" \
  "Journey D — logout vault teardown"
assert_contains "$ROOT/docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md" \
  "Rotated-session logout refreshes, resumes teardown, removes vault/key, and retains floor"
assert_contains "$ROOT/docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md" \
  "scripts/release/candidate-transport.sh"
assert_contains "$ROOT/docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md" \
  "HEYFOOD_APPROVED_CANDIDATE_RUN_ID"
assert_contains "$ROOT/RELEASING.md" \
  "The tag workflow never rebuilds, re-signs, repackages, or regenerates the"
assert_contains "$ROOT/RELEASING.md" \
  "complete public set contains exactly ten files"
assert_contains "$ROOT/RELEASING.md" \
  "HEYFOOD_APPROVED_CANDIDATE_SHA256SUMS_SHA256"
assert_contains "$ROOT/docs/NATIVE_STATE_COMPATIBILITY.md" \
  "Invoking it with \`HEYFOOD_VERSION=0.6.2\` is rejected before"
assert_contains "$ROOT/docs/NATIVE_STATE_COMPATIBILITY.md" \
  "archived v0.6.2 installer and binary"
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
jq -e '
  .distribution.release_assets == {
    "product_archives": 4,
    "verifier_archives": 4,
    "native_state_declarations": 1,
    "checksum_manifests": 1,
    "checksum_entries": 9,
    "total_public_files": 10
  } and
  (.deferred_household_capabilities | index("hosted_member_guidance_and_evaluation")) != null and
  (.deferred_household_capabilities | index("cross_device_household_state")) != null
' "$ROOT/tests/showcase/core-release-matrix.v1.json" >/dev/null ||
  fail "the installed-artifact matrix must bind the v$version native-state release boundary"

for source in "$ROOT/README.md" "$ROOT/docs/CAPABILITY_STATUS.md"; do
  if grep -Eq 'v0\.4\.[01]' "$source"; then
    fail "$source must not advertise obsolete incident releases"
  fi
done

printf 'version documentation contract: v%s coordinated\n' "$version"
