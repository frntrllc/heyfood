#!/usr/bin/env bash
# shellcheck disable=SC2016 # Contract checks intentionally match literal workflow source.

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
[[ "$(grep -Fc 'packaging/macos/sign-and-notarize.sh' "$protected_slice")" -eq 2 ]] ||
  fail "protected qualification must sign and notarize both macOS executables"
for subject in \
  'candidate-release/*.tar.gz' \
  'candidate-release/*.json' \
  'candidate-release/SHA256SUMS'; do
  grep -Fq "$subject" "$protected_slice" ||
    fail "protected qualification must attest $subject"
done

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
  "$ROOT/scripts/release/package.sh" "$ROOT/install.sh" 0.6.2 "$target" "$distribution"
done
"$ROOT/scripts/release/checksums.sh" "$distribution" 0.6.2
"$ROOT/scripts/release/verify-assets.sh" "$distribution" 0.6.2
[[ "$(wc -l <"$distribution/SHA256SUMS" | tr -d '[:space:]')" -eq 4 ]] ||
  fail "the immutable v0.6.2 manifest must bind exactly four product archives"
[[ ! -e "$distribution/heyfood-v0.6.2-native-state.json" ]] ||
  fail "release tooling must not invent a declaration for immutable v0.6.2"
if find "$distribution" -maxdepth 1 -type f \
  -name 'heyfood-installer-v0.6.2-*.tar.gz' | grep -q .; then
  fail "release tooling must not invent verifier archives for immutable v0.6.2"
fi
[[ "$(find "$distribution" -maxdepth 1 -type f | wc -l | tr -d '[:space:]')" -eq 5 ]] ||
  fail "immutable v0.6.2 must remain four product archives plus SHA256SUMS"

native_state_distribution="$CASE_DIR/native-state-distribution"
mkdir "$native_state_distribution"
for target in \
  aarch64-apple-darwin \
  aarch64-unknown-linux-gnu \
  x86_64-apple-darwin \
  x86_64-unknown-linux-gnu; do
  "$ROOT/scripts/release/package.sh" \
    "$ROOT/install.sh" \
    0.6.3 \
    "$target" \
    "$native_state_distribution"
  "$ROOT/scripts/release/package-installer.sh" \
    "$ROOT/install.sh" \
    0.6.3 \
    "$target" \
    "$native_state_distribution"
done
"$ROOT/scripts/release/checksums.sh" \
  "$native_state_distribution" 0.6.3 --native-state
"$ROOT/scripts/release/verify-assets.sh" \
  "$native_state_distribution" 0.6.3 --native-state
[[ "$(wc -l <"$native_state_distribution/SHA256SUMS" | tr -d '[:space:]')" -eq 9 ]] ||
  fail "v0.6.3 must bind four product archives, four verifier archives, and one declaration"
[[ -f "$native_state_distribution/heyfood-v0.6.3-native-state.json" ]] ||
  fail "v0.6.3 must contain the canonical native-state declaration"
[[ "$(find "$native_state_distribution" -maxdepth 1 -type f | wc -l | tr -d '[:space:]')" -eq 10 ]] ||
  fail "v0.6.3 must contain exactly ten public files including SHA256SUMS"

windows_asset="$distribution/heyfood-v0.6.2-x86_64-pc-windows-msvc.zip"
touch "$windows_asset"
if "$ROOT/scripts/release/checksums.sh" "$distribution" 0.6.2 >/dev/null 2>&1; then
  fail "checksum generation must reject a Windows v0.6.2 asset"
fi
if "$ROOT/scripts/release/verify-assets.sh" "$distribution" 0.6.2 >/dev/null 2>&1; then
  fail "complete-set verification must reject a Windows v0.6.2 asset"
fi

grep -Fq -- '--package heyfood-installer' "$RELEASE_WORKFLOW" ||
  fail "the release workflow must build the standalone verifier"
grep -Fq 'scripts/release/package-installer.sh' "$RELEASE_WORKFLOW" ||
  fail "the release workflow must package the standalone verifier"
grep -Fq "target/\$TARGET/release/heyfood-installer" "$RELEASE_WORKFLOW" ||
  fail "the release workflow must sign and smoke the target verifier bytes"
grep -Fq 'dist/*.json' "$RELEASE_WORKFLOW" ||
  fail "the release workflow must attest the native-state declaration"
grep -Fq 'dist/SHA256SUMS' "$RELEASE_WORKFLOW" ||
  fail "the release workflow must attest the checksum manifest"
grep -Fq 'dist/heyfood-v${{ needs.validate.outputs.version }}-${{ matrix.target }}.tar.gz' \
  "$RELEASE_WORKFLOW" ||
  fail "the release workflow must upload each product archive explicitly"
grep -Fq 'dist/heyfood-installer-v${{ needs.validate.outputs.version }}-${{ matrix.target }}.tar.gz' \
  "$RELEASE_WORKFLOW" ||
  fail "the release workflow must upload each verifier archive explicitly"
[[ "$(grep -Fc 'packaging/macos/sign-and-notarize.sh' "$RELEASE_WORKFLOW")" -eq 2 ]] ||
  fail "the release workflow must sign and notarize both macOS executables"
grep -Fq 'scripts/release/smoke-archive.sh dist "$VERSION" "$TARGET"' \
  "$RELEASE_WORKFLOW" ||
  fail "the release workflow must smoke each final product/verifier pair"
grep -Fq 'test "${#assets[@]}" -eq 10' "$PUBLIC_SMOKE_WORKFLOW" ||
  fail "public smoke must require all ten v0.6.3 assets"
grep -Fq 'gh attestation verify "$asset"' "$PUBLIC_SMOKE_WORKFLOW" ||
  fail "public smoke must verify the attestation for every downloaded asset"
grep -Fq 'scripts/release/smoke.sh' "$PUBLIC_SMOKE_WORKFLOW" ||
  fail "public smoke must execute the product and verifier archive pair"

ordinary_distribution_slice="$CASE_DIR/ordinary-distribution-ci.yml"
sed -n '/^  native-release-contract:/,/^  protected-candidate-preflight:/p' \
  "$CANDIDATE_WORKFLOW" >"$ordinary_distribution_slice"
grep -Fq -- '--package heyfood-installer' "$ordinary_distribution_slice" ||
  fail "ordinary distribution CI must build the standalone verifier"
grep -Fq 'scripts/release/package-installer.sh' "$ordinary_distribution_slice" ||
  fail "ordinary distribution CI must package verifier fixtures"
grep -Fq 'scripts/release/verify-assets.sh dist "$version" --native-state' \
  "$ordinary_distribution_slice" ||
  fail "ordinary distribution CI must verify the complete native-state fixture set"

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
  .distribution.release_assets == {
    "product_archives": 4,
    "verifier_archives": 4,
    "native_state_declarations": 1,
    "checksum_manifests": 1,
    "checksum_entries": 9,
    "total_public_files": 10
  } and
  .distribution.immutable_v0_6_2 == {
    "product_archives": 4,
    "verifier_archives": 0,
    "native_state_declarations": 0,
    "checksum_manifests": 1
  } and
  .explicit_non_gates == [
    "native_voice",
    "menu_watch_diff",
    "health_integrations"
  ] and
  .deferred_household_capabilities == [
    "hosted_member_guidance_and_evaluation",
    "member_profile_sync",
    "learned_dietary_graph",
    "member_health_and_fitness_data",
    "cross_device_household_state",
    "remote_member_erasure"
  ] and
  .manual_release_gates == [
    "clean_v0_6_3_install",
    "v0_6_2_to_v0_6_3_upgrade",
    "pre_native_state_downgrade_floor_refusal",
    "authorization_rollover_preserves_household_binding",
    "logout_removes_account_vault_and_preserves_global_floor"
  ]
' "$ROOT/tests/showcase/core-release-matrix.v1.json" >/dev/null ||
  fail "the core matrix must preserve the bounded distribution and non-gates"

printf 'release scope contract: complete v0.6.3 native-state set; Windows CI retained\n'
