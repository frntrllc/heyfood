#!/usr/bin/env bash

set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT
readonly RELEASE_WORKFLOW="$ROOT/.github/workflows/release.yml"
readonly CANDIDATE_WORKFLOW="$ROOT/.github/workflows/ci.yml"
readonly PUBLIC_SMOKE_WORKFLOW="$ROOT/.github/workflows/post-release-smoke.yml"
CASE_DIR=$(mktemp -d)
readonly CASE_DIR

cleanup() {
  rm -rf -- "$CASE_DIR"
}
trap cleanup EXIT

fail() {
  printf 'release scope contract: %s\n' "$*" >&2
  exit 1
}

assert_four_targets() {
  local source=$1
  local target
  local count
  count=$(grep -Ec \
    '^[[:space:]]+target: (aarch64-apple-darwin|aarch64-unknown-linux-gnu|x86_64-apple-darwin|x86_64-unknown-linux-gnu)$' \
    "$source")
  [[ "$count" -eq 4 ]] || fail "$source must define exactly four release targets"
  for target in \
    aarch64-apple-darwin \
    aarch64-unknown-linux-gnu \
    x86_64-apple-darwin \
    x86_64-unknown-linux-gnu; do
    [[ "$(grep -Fc "target: $target" "$source")" -eq 1 ]] ||
      fail "$source must define $target exactly once"
  done
}

assert_no_windows_release_path() {
  local source=$1
  if grep -Eq \
    'x86_64-pc-windows-msvc|windows-2025|WINDOWS_CODESIGN|WINDOWS_TIMESTAMP|packaging/windows|\.zip' \
    "$source"; then
    fail "$source must not contain Windows release assets or credentials"
  fi
}

line_of() {
  local source=$1
  local pattern=$2
  awk -v pattern="$pattern" 'index($0, pattern) { print NR; exit }' "$source"
}

protected_slice="$CASE_DIR/protected-ci.yml"
sed -n '/^  protected-candidate-preflight:/,$p' "$CANDIDATE_WORKFLOW" >"$protected_slice"
macos_signer="$ROOT/packaging/macos/sign-and-notarize.sh"
archive_smoke="$ROOT/scripts/release/smoke-archive.sh"
agent_setup_smoke="$ROOT/scripts/release/agent-setup-smoke.sh"
mcp_smoke="$ROOT/scripts/release/mcp-smoke.mjs"

[[ -x "$macos_signer" ]] ||
  fail "the macOS signing tool must be executable"
[[ "$(git -C "$ROOT" ls-files --stage -- packaging/macos/sign-and-notarize.sh |
  awk '{print $1}')" == "100755" ]] ||
  fail "Git must record the macOS signing tool with mode 100755"
grep -Fq '[.commands[].path] | index("mcp serve")' "$archive_smoke" ||
  fail "archive smoke must validate the documented command path field"
if grep -Fq '[.commands[].name] | index("mcp serve")' "$archive_smoke"; then
  fail "archive smoke must not validate the absent command name field"
fi
grep -Fq 'mkdirSync(join(cleanHome, "Library", "Preferences")' "$mcp_smoke" ||
  fail "macOS MCP smoke must create the clean profile preference root"
grep -Fq 'process.env.HEYFOOD_QUALIFICATION_KEYCHAIN' "$mcp_smoke" ||
  fail "macOS MCP smoke must bind an externally managed qualification keychain"
grep -Fq '["default-keychain", "-d", "user", "-s", keychain]' "$mcp_smoke" ||
  fail "macOS MCP smoke must set the clean profile default keychain"
grep -Fq '["list-keychains", "-d", "user", "-s", keychain]' "$mcp_smoke" ||
  fail "macOS MCP smoke must restrict the clean profile keychain search list"
grep -Fq '["delete-keychain", ownedMacKeychain]' "$mcp_smoke" ||
  fail "self-contained macOS MCP smoke must destroy its ephemeral keychain"

# shellcheck disable=SC2016 # These are literal source patterns, not expansions.
create_smoke_root_line=$(line_of "$agent_setup_smoke" 'mkdir -p -- "$root"')
# shellcheck disable=SC2016 # These are literal source patterns, not expansions.
canonicalize_smoke_root_line=$(line_of "$agent_setup_smoke" 'root=$(cd "$root" && pwd -P)')
if [[ -z "$create_smoke_root_line" || -z "$canonicalize_smoke_root_line" ||
  "$create_smoke_root_line" -ge "$canonicalize_smoke_root_line" ]]; then
  fail "agent setup archive smoke must create its fresh root before canonicalizing it"
fi

capture_line=$(line_of "$macos_signer" 'security list-keychains -d user |')
create_line=$(line_of "$macos_signer" 'security create-keychain')
settings_line=$(line_of "$macos_signer" 'security set-keychain-settings')
unlock_line=$(line_of "$macos_signer" 'security unlock-keychain')
import_line=$(line_of "$macos_signer" "security import \"\$p12\"")
partition_line=$(line_of "$macos_signer" 'security set-key-partition-list')
changed_line=$(line_of "$macos_signer" 'keychain_search_list_changed=true')
register_line=$(line_of "$macos_signer" \
  "security list-keychains -d user -s \"\$keychain\"")
identity_line=$(line_of "$macos_signer" 'security find-identity')
if [[ -z "$capture_line" || -z "$create_line" || -z "$settings_line" ||
  -z "$unlock_line" || -z "$import_line" || -z "$partition_line" ||
  -z "$changed_line" || -z "$register_line" || -z "$identity_line" ]]; then
  fail "the macOS signing tool must contain the hosted-runner keychain sequence"
fi
if ! ((capture_line < create_line &&
  create_line < settings_line &&
  settings_line < unlock_line &&
  unlock_line < import_line &&
  import_line < partition_line &&
  partition_line < changed_line &&
  changed_line < register_line &&
  register_line < identity_line)); then
  fail "the macOS keychain sequence must be capture, create, configure, import, partition, register, discover"
fi
partition_block=$(sed -n \
  "/^security set-key-partition-list \\\\/,/^  \"\\\$keychain\"\$/p" \
  "$macos_signer")
if grep -Eq '^[[:space:]]+-s([[:space:]\\]|$)' <<<"$partition_block"; then
  fail "the hosted macOS partition-list command must not use the failing -s predicate"
fi
grep -Fq "security list-keychains -d user -s \"\${original_keychains[@]}\"" \
  "$macos_signer" ||
  fail "macOS signing cleanup must restore the prior user keychain search list"
restore_line=$(line_of "$macos_signer" \
  "security list-keychains -d user -s \"\${original_keychains[@]}\"")
delete_line=$(line_of "$macos_signer" "security delete-keychain \"\$keychain\"")
if [[ -z "$restore_line" || -z "$delete_line" || "$restore_line" -ge "$delete_line" ]]; then
  fail "macOS signing cleanup must restore the search list before deleting the keychain"
fi
if grep -Eq '(^|[[:space:]])spctl([[:space:]]|$)' "$macos_signer"; then
  fail "standalone macOS executables must not use spctl app assessment"
fi
if grep -Eq '(^|[[:space:]])spctl([[:space:]]|$)' "$archive_smoke"; then
  fail "packaged standalone macOS executables must not use spctl app assessment"
fi
accepted_line=$(line_of "$macos_signer" 'result.get("status") != "Accepted"')
submission_evidence_line=$(line_of "$macos_signer" '"submission_id": submission_id')
notarized_code_line=$(line_of "$macos_signer" \
  "codesign -vvvv -R=\"notarized\" --check-notarization \"\$binary\"")
if [[ -z "$accepted_line" || -z "$submission_evidence_line" ||
  -z "$notarized_code_line" ]]; then
  fail "macOS signing must retain sanitized acceptance evidence and check notarized standalone code"
fi
if ! ((accepted_line < submission_evidence_line &&
  submission_evidence_line < notarized_code_line)); then
  fail "macOS signing must validate Accepted, log its submission ID, then check notarized code"
fi
grep -Fq "codesign -vvvv -R=\"notarized\" --check-notarization \"\$binary\"" \
  "$archive_smoke" ||
  fail "archive smoke must check the notarization of packaged standalone macOS code"

assert_four_targets "$RELEASE_WORKFLOW"
assert_four_targets "$PUBLIC_SMOKE_WORKFLOW"
assert_four_targets "$protected_slice"
assert_no_windows_release_path "$RELEASE_WORKFLOW"
assert_no_windows_release_path "$PUBLIC_SMOKE_WORKFLOW"
assert_no_windows_release_path "$protected_slice"
grep -Fq "if [[ -z \"\${HEYFOOD_QUALIFICATION_KEYCHAIN:-}\" ]]; then" "$protected_slice" ||
  fail "protected cleanup must tolerate a keychain that was never created"
grep -Fq "if: \${{ always() && hashFiles('candidate-dist/**', 'candidate-evidence/**') != '' }}" \
  "$protected_slice" ||
  fail "protected upload must run only when candidate output exists"

grep -Fq 'os: [ubuntu-22.04, macos-15, windows-2025]' "$CANDIDATE_WORKFLOW" ||
  fail "ordinary Windows CI must remain enabled"
grep -Fq 'Package, smoke, and reproduce the Windows release archive' "$CANDIDATE_WORKFLOW" ||
  fail "ordinary Windows packaging qualification must remain enabled"
grep -Fq 'Test the default product feature set on Windows without the process-global console case' \
  "$ROOT/.github/workflows/rust-ci.yml" ||
  fail "ordinary Rust Windows tests must remain enabled"
grep -Fq 'Test the process-global Windows console case in isolation' \
  "$ROOT/.github/workflows/rust-ci.yml" ||
  fail "ordinary Rust Windows console lifecycle tests must remain enabled"

distribution="$CASE_DIR/distribution"
mkdir "$distribution"
for target in \
  aarch64-apple-darwin \
  aarch64-unknown-linux-gnu \
  x86_64-apple-darwin \
  x86_64-unknown-linux-gnu; do
  "$ROOT/scripts/release/package.sh" "$ROOT/install.sh" 0.6.3 "$target" "$distribution"
done
"$ROOT/scripts/release/checksums.sh" "$distribution" 0.6.3
"$ROOT/scripts/release/verify-assets.sh" "$distribution" 0.6.3
[[ "$(wc -l <"$distribution/SHA256SUMS" | tr -d '[:space:]')" -eq 4 ]] ||
  fail "the release manifest must bind exactly four archives"

windows_asset="$distribution/heyfood-v0.6.3-x86_64-pc-windows-msvc.zip"
touch "$windows_asset"
if "$ROOT/scripts/release/checksums.sh" "$distribution" 0.6.3 >/dev/null 2>&1; then
  fail "checksum generation must reject a Windows v0.6.3 asset"
fi
if "$ROOT/scripts/release/verify-assets.sh" "$distribution" 0.6.3 >/dev/null 2>&1; then
  fail "complete-set verification must reject a Windows v0.6.3 asset"
fi

grep -Fq "Windows distribution remains deferred" "$ROOT/README.md" ||
  fail "README must state the Windows release boundary"
grep -Fq "Windows distribution requires a separately qualified future release" "$ROOT/docs/CAPABILITY_STATUS.md" ||
  fail "capability status must state the Windows release boundary"
grep -Fq "Windows distribution is deferred to a separately qualified future release" "$ROOT/docs/RELEASE_SIGNING.md" ||
  fail "signing policy must state the Windows release boundary"
jq -e '
  .distribution.release_targets == [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu"
  ] and
  .distribution.windows_distribution == "deferred_to_future_release" and
  .distribution.ordinary_windows_ci_required == true and
  .explicit_non_gates == [
    "native_voice",
    "menu_watch_diff",
    "health_integrations"
  ]
' "$ROOT/tests/showcase/core-release-matrix.v1.json" >/dev/null ||
  fail "the core matrix must preserve the bounded distribution and non-gates"

printf 'release scope contract: four v0.6.3 archives; Windows CI retained\n'
