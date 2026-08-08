#!/usr/bin/env bash
# shellcheck disable=SC2016 # Contract checks intentionally match literal workflow source.

set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT
readonly RELEASE_WORKFLOW="$ROOT/.github/workflows/release.yml"
readonly CANDIDATE_WORKFLOW="$ROOT/.github/workflows/ci.yml"
readonly PUBLIC_SMOKE_WORKFLOW="$ROOT/.github/workflows/post-release-smoke.yml"
WORKSPACE_VERSION=$(cargo metadata --locked --no-deps --format-version 1 \
  --manifest-path "$ROOT/Cargo.toml" | jq -er \
  '.packages[] | select(.name == "heyfood-bin") | .version')
readonly WORKSPACE_VERSION
SUPPORTED_RELEASE_VERSION=$(jq -er '.release' \
  "$ROOT/tests/showcase/core-release-matrix.v1.json")
readonly SUPPORTED_RELEASE_VERSION
CURRENT_GATE_VERSION="v${SUPPORTED_RELEASE_VERSION//./_}"
readonly CURRENT_GATE_VERSION
readonly PREVIOUS_GATE_VERSION="v0_7_1"
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
candidate_transport="$ROOT/scripts/release/candidate-transport.sh"
asset_verifier="$ROOT/scripts/release/verify-assets.sh"

[[ -x "$macos_signer" ]] ||
  fail "the macOS signing tool must be executable"
[[ "$(git -C "$ROOT" ls-files --stage -- packaging/macos/sign-and-notarize.sh |
  awk '{print $1}')" == "100755" ]] ||
  fail "Git must record the macOS signing tool with mode 100755"
grep -Fq '[.commands[].path] | index("mcp serve")' "$archive_smoke" ||
  fail "archive smoke must validate the documented command path field"
grep -Fq '.schema_version == 3' "$archive_smoke" ||
  fail "archive smoke must validate the current schema-v3 manifest"
for household_tool in heyfood_get_household_context heyfood_get_household_member; do
  grep -Fq "$household_tool" "$archive_smoke" ||
    fail "archive smoke must validate $household_tool"
  grep -Fq "$household_tool" "$mcp_smoke" ||
    fail "MCP smoke must validate $household_tool"
done
grep -Fq 'schema_version !== 3' "$mcp_smoke" ||
  fail "MCP smoke must validate the current schema-v3 manifest"
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

assert_four_targets "$PUBLIC_SMOKE_WORKFLOW"
assert_four_targets "$protected_slice"
assert_no_windows_release_path "$RELEASE_WORKFLOW"
assert_no_windows_release_path "$PUBLIC_SMOKE_WORKFLOW"
assert_no_windows_release_path "$protected_slice"
v2_manifest_line=$(line_of "$protected_slice" 'agent describe --schema-version 2')
verifier_line=$(line_of "$protected_slice" '"$verifier" verify-native-state')
if [[ -z "$v2_manifest_line" || -z "$verifier_line" ||
  "$v2_manifest_line" -ge "$verifier_line" ]]; then
  fail "protected qualification must request the closed v2 manifest before native-state verification"
fi
grep -Fq '.schema_version == 2' "$protected_slice" ||
  fail "protected qualification must assert the explicit candidate manifest schema"
if grep -Fq 'agent describe >candidate-agent-manifest.json' "$protected_slice"; then
  fail "protected qualification must not pass the default v1 manifest to the v2-only verifier"
fi
grep -Fq 'HEYFOOD_DISTRIBUTION_CHANNEL: release' "$protected_slice" ||
  fail "protected candidate binaries must embed the release distribution channel"
grep -Fq '.build.distribution_channel == "release"' "$protected_slice" ||
  fail "protected qualification must assert the release distribution channel"
if grep -Fq '.build.distribution_channel == "candidate"' "$protected_slice"; then
  fail "protected publication candidates must not embed the candidate channel"
fi
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

if grep -Eq 'windows-(latest|2025)|runner\.os.*Windows|x86_64-pc-windows-msvc' \
  "$CANDIDATE_WORKFLOW" "$ROOT/.github/workflows/rust-ci.yml"; then
  fail "the accelerated v0.8.0 blocking CI graph must remain macOS/Linux-only"
fi
if grep -Fq 'Package, smoke, and reproduce the Windows release archive' \
  "$CANDIDATE_WORKFLOW"; then
  fail "Windows packaging qualification must remain outside the v0.8.0 release path"
fi

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
    0.8.0 \
    "$target" \
    "$native_state_distribution"
  "$ROOT/scripts/release/package-installer.sh" \
    "$ROOT/install.sh" \
    0.8.0 \
    "$target" \
    "$native_state_distribution"
done
"$ROOT/scripts/release/checksums.sh" \
  "$native_state_distribution" 0.8.0 --native-state
"$ROOT/scripts/release/verify-assets.sh" \
  "$native_state_distribution" 0.8.0 --native-state
[[ "$(wc -l <"$native_state_distribution/SHA256SUMS" | tr -d '[:space:]')" -eq 9 ]] ||
  fail "v0.8.0 must bind four product archives, four verifier archives, and one declaration"
[[ -f "$native_state_distribution/heyfood-v0.8.0-native-state.json" ]] ||
  fail "v0.8.0 must contain the canonical native-state declaration"
[[ "$(find "$native_state_distribution" -maxdepth 1 -type f | wc -l | tr -d '[:space:]')" -eq 10 ]] ||
  fail "v0.8.0 must contain exactly ten public files including SHA256SUMS"

[[ -x "$candidate_transport" ]] ||
  fail "the content-free candidate transport must be executable"
[[ "$(git -C "$ROOT" ls-files --stage -- scripts/release/candidate-transport.sh |
  awk '{print $1}')" == "100755" ]] ||
  fail "Git must record the content-free candidate transport with mode 100755"
grep -Fq 'scripts/release/candidate-transport.sh' \
  "$ROOT/docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md" ||
  fail "manual household acceptance must use the checked-in candidate transport"
grep -Fq -- '--expect-no-download 0.7.1' \
  "$ROOT/docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md" ||
  fail "Journey B must document the exact no-download v0.7.1 refusal mode"
grep -Fq -- '--native-state-manifest-bound' "$candidate_transport" ||
  fail "candidate transport must use network-free manifest-bound verification"
grep -Fq 'native-state-declaration.sh' "$asset_verifier" ||
  fail "protected full verification must retain declaration regeneration"

candidate_manifest_sha256=$(shasum -a 256 \
  "$native_state_distribution/SHA256SUMS" | awk '{print $1}')
prohibited_tool_bin="$CASE_DIR/prohibited-candidate-tools"
prohibited_tool_markers="$CASE_DIR/prohibited-candidate-tool-markers"
mkdir -p -- "$prohibited_tool_bin" "$prohibited_tool_markers"
cat >"$prohibited_tool_bin/prohibited-tool" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
: "${HEYFOOD_PROHIBITED_TOOL_MARKER_DIR:?}"
tool=${0##*/}
printf 'invoked\n' >"$HEYFOOD_PROHIBITED_TOOL_MARKER_DIR/$tool"
printf 'prohibited candidate tool invoked: %s\n' "$tool" >&2
exit 97
EOF
chmod 0755 "$prohibited_tool_bin/prohibited-tool"
for prohibited_tool in cargo rustup curl; do
  ln -s "$prohibited_tool_bin/prohibited-tool" \
    "$prohibited_tool_bin/$prohibited_tool"
done

assert_candidate_tools_unused() {
  local prohibited_tool
  for prohibited_tool in cargo rustup curl; do
    [[ ! -e "$prohibited_tool_markers/$prohibited_tool" ]] ||
      fail "candidate verification invoked prohibited $prohibited_tool"
  done
}

PATH="$prohibited_tool_bin:$PATH" \
  HEYFOOD_PROHIBITED_TOOL_MARKER_DIR="$prohibited_tool_markers" \
  "$asset_verifier" \
  "$native_state_distribution" \
  0.8.0 \
  --native-state-manifest-bound \
  "$candidate_manifest_sha256"
assert_candidate_tools_unused

duplicate_manifest_distribution="$CASE_DIR/duplicate-manifest-distribution"
mkdir "$duplicate_manifest_distribution"
cp -R "$native_state_distribution/." "$duplicate_manifest_distribution/"
duplicate_manifest_entry=$(sed -n '1p' \
  "$duplicate_manifest_distribution/SHA256SUMS")
printf '%s\n' "$duplicate_manifest_entry" \
  >>"$duplicate_manifest_distribution/SHA256SUMS"
duplicate_manifest_sha256=$(shasum -a 256 \
  "$duplicate_manifest_distribution/SHA256SUMS" | awk '{print $1}')
if PATH="$prohibited_tool_bin:$PATH" \
  HEYFOOD_PROHIBITED_TOOL_MARKER_DIR="$prohibited_tool_markers" \
  "$asset_verifier" \
  "$duplicate_manifest_distribution" \
  0.8.0 \
  --native-state-manifest-bound \
  "$duplicate_manifest_sha256" >/dev/null 2>&1; then
  fail "manifest-bound verification accepted a duplicate checksum entry"
fi
assert_candidate_tools_unused

tampered_candidate_distribution="$CASE_DIR/tampered-candidate-distribution"
mkdir "$tampered_candidate_distribution"
cp -R "$native_state_distribution/." "$tampered_candidate_distribution/"
printf 'tampered candidate bytes\n' \
  >>"$tampered_candidate_distribution/heyfood-v0.8.0-aarch64-apple-darwin.tar.gz"
if PATH="$prohibited_tool_bin:$PATH" \
  HEYFOOD_PROHIBITED_TOOL_MARKER_DIR="$prohibited_tool_markers" \
  "$asset_verifier" \
  "$tampered_candidate_distribution" \
  0.8.0 \
  --native-state-manifest-bound \
  "$candidate_manifest_sha256" >/dev/null 2>&1; then
  fail "manifest-bound verification accepted changed candidate bytes"
fi
assert_candidate_tools_unused

unexpected_candidate_distribution="$CASE_DIR/unexpected-candidate-distribution"
mkdir "$unexpected_candidate_distribution"
cp -R "$native_state_distribution/." "$unexpected_candidate_distribution/"
printf 'unexpected\n' >"$unexpected_candidate_distribution/.unexpected.asset"
if PATH="$prohibited_tool_bin:$PATH" \
  HEYFOOD_PROHIBITED_TOOL_MARKER_DIR="$prohibited_tool_markers" \
  "$asset_verifier" \
  "$unexpected_candidate_distribution" \
  0.8.0 \
  --native-state-manifest-bound \
  "$candidate_manifest_sha256" >/dev/null 2>&1; then
  fail "manifest-bound verification accepted an eleventh release file"
fi
assert_candidate_tools_unused

candidate_transport_output="$CASE_DIR/candidate-transport-output"
candidate_transport_installer="$CASE_DIR/candidate-transport-installer.sh"
cat >"$candidate_transport_installer" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
: "${HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT:?}"
: "${HEYFOOD_VERSION:?}"
mkdir -p -- "$HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT"
prefix="https://github.com/frntrllc/heyfood/releases/download/v$HEYFOOD_VERSION"
curl -qfsSL \
  --proto '=https' \
  --tlsv1.2 \
  --retry 3 \
  --output "$HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT/SHA256SUMS" \
  "$prefix/SHA256SUMS"
curl -qfsSL \
  --proto '=https' \
  --tlsv1.2 \
  --retry 3 \
  --output "$HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT/product.tar.gz" \
  "$prefix/heyfood-v$HEYFOOD_VERSION-x86_64-unknown-linux-gnu.tar.gz"
EOF
chmod 0755 "$candidate_transport_installer"
PATH="$prohibited_tool_bin:$PATH" \
  HEYFOOD_PROHIBITED_TOOL_MARKER_DIR="$prohibited_tool_markers" \
  HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT="$candidate_transport_output" \
  "$candidate_transport" \
  "$native_state_distribution" \
  0.8.0 \
  "$candidate_manifest_sha256" \
  "$candidate_transport_installer"
assert_candidate_tools_unused
cmp \
  "$native_state_distribution/SHA256SUMS" \
  "$candidate_transport_output/SHA256SUMS"
cmp \
  "$native_state_distribution/heyfood-v0.8.0-x86_64-unknown-linux-gnu.tar.gz" \
  "$candidate_transport_output/product.tar.gz"

refusal_root="$CASE_DIR/candidate-transport-refusal"
refusal_home="$refusal_root/home"
refusal_bin="$refusal_root/bin"
refusal_state="$refusal_root/state"
refusal_floor="$refusal_state/data/compatibility/native-state-floor.v1.json"
refusal_vault="$refusal_state/data/accounts/test-account/household-state.enc"
refusal_stdout="$refusal_root/stdout"
refusal_stderr="$refusal_root/stderr"
expected_refusal="$refusal_root/expected.stderr"
mkdir -p -- \
  "$refusal_home" \
  "$refusal_bin" \
  "$(dirname "$refusal_floor")" \
  "$(dirname "$refusal_vault")"
chmod 0700 \
  "$refusal_home" \
  "$refusal_bin" \
  "$refusal_state" \
  "$refusal_state/data" \
  "$(dirname "$refusal_floor")" \
  "$refusal_state/data/accounts" \
  "$(dirname "$refusal_vault")"
printf '#!/usr/bin/env bash\nprintf "heyfood 0.8.0\\n"\n' \
  >"$refusal_bin/heyfood"
printf 'native-state floor sentinel\n' >"$refusal_floor"
printf 'encrypted household state sentinel\n' >"$refusal_vault"
chmod 0755 "$refusal_bin/heyfood"
chmod 0600 "$refusal_floor" "$refusal_vault"
cp "$refusal_bin/heyfood" "$refusal_root/heyfood.before"
cp "$refusal_floor" "$refusal_root/floor.before"
cp "$refusal_vault" "$refusal_root/vault.before"
printf '%s\n' \
  'heyfood installer: this installer supports heyfood 0.8.0; requested 0.7.1' \
  >"$expected_refusal"

refusal_status=0
HOME="$refusal_home" \
  HEYFOOD_BIN_DIR="$refusal_bin" \
  HEYFOOD_STATE_DIR="$refusal_state" \
  PATH="$prohibited_tool_bin:$PATH" \
  HEYFOOD_PROHIBITED_TOOL_MARKER_DIR="$prohibited_tool_markers" \
  "$candidate_transport" \
  --expect-no-download 0.7.1 \
  "$native_state_distribution" \
  0.8.0 \
  "$candidate_manifest_sha256" \
  "$ROOT/install.sh" >"$refusal_stdout" 2>"$refusal_stderr" ||
  refusal_status=$?
assert_candidate_tools_unused
[[ "$refusal_status" -eq 1 ]] ||
  fail "the candidate refusal fixture did not preserve installer exit status 1"
[[ -z "$(find "$refusal_home" -mindepth 1 -maxdepth 1 -print -quit)" ]] ||
  fail "the candidate refusal fixture mutated the isolated HOME"
[[ ! -s "$refusal_stdout" ]] ||
  fail "the current installer wrote stdout before refusing requested v0.7.1"
cmp "$expected_refusal" "$refusal_stderr" ||
  fail "the current installer did not receive requested v0.7.1 or emit the exact refusal"
cmp "$refusal_root/heyfood.before" "$refusal_bin/heyfood" ||
  fail "the no-download refusal changed the installed executable sentinel"
cmp "$refusal_root/floor.before" "$refusal_floor" ||
  fail "the no-download refusal changed the native-state floor sentinel"
cmp "$refusal_root/vault.before" "$refusal_vault" ||
  fail "the no-download refusal changed the encrypted household state sentinel"

prohibited_download_installer="$CASE_DIR/prohibited-download-installer.sh"
prohibited_download_output="$CASE_DIR/prohibited-download-output"
prohibited_download_stderr="$CASE_DIR/prohibited-download.stderr"
cat >"$prohibited_download_installer" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
: "${HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT:?}"
: "${HEYFOOD_VERSION:?}"
[[ "$HEYFOOD_VERSION" == "0.7.1" ]]
curl -qfsSL \
  --proto '=https' \
  --tlsv1.2 \
  --retry 3 \
  --output "$HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT" \
  "https://github.com/frntrllc/heyfood/releases/download/v0.8.0/SHA256SUMS"
EOF
chmod 0755 "$prohibited_download_installer"
if PATH="$prohibited_tool_bin:$PATH" \
  HEYFOOD_PROHIBITED_TOOL_MARKER_DIR="$prohibited_tool_markers" \
  HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT="$prohibited_download_output" \
  "$candidate_transport" \
  --expect-no-download 0.7.1 \
  "$native_state_distribution" \
  0.8.0 \
  "$candidate_manifest_sha256" \
  "$prohibited_download_installer" >/dev/null 2>"$prohibited_download_stderr"; then
  fail "expect-no-download mode accepted an installer curl invocation"
fi
grep -Fq \
  'candidate transport: the installer attempted a download in expect-no-download mode' \
  "$prohibited_download_stderr" ||
  fail "expect-no-download mode did not fail closed on curl invocation"
[[ ! -e "$prohibited_download_output" && ! -L "$prohibited_download_output" ]] ||
  fail "expect-no-download mode served candidate bytes"
assert_candidate_tools_unused

rejected_transport_output="$CASE_DIR/rejected-candidate-transport-output"
if HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT="$rejected_transport_output" \
  "$candidate_transport" \
  "$native_state_distribution" \
  0.8.0 \
  "$(printf '0%.0s' {1..64})" \
  "$candidate_transport_installer" >/dev/null 2>&1; then
  fail "the candidate transport accepted an unapproved release-set digest"
fi
[[ ! -e "$rejected_transport_output" ]] ||
  fail "the candidate transport invoked the installer before approving the release-set digest"

unapproved_url_installer="$CASE_DIR/unapproved-url-installer.sh"
cat >"$unapproved_url_installer" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
: "${HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT:?}"
mkdir -p -- "$HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT"
curl -qfsSL \
  --proto '=https' \
  --tlsv1.2 \
  --retry 3 \
  --output "$HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT/unapproved" \
  "https://example.com/SHA256SUMS"
EOF
chmod 0755 "$unapproved_url_installer"
unapproved_url_output="$CASE_DIR/unapproved-url-output"
if HEYFOOD_CANDIDATE_TRANSPORT_TEST_OUTPUT="$unapproved_url_output" \
  "$candidate_transport" \
  "$native_state_distribution" \
  0.8.0 \
  "$candidate_manifest_sha256" \
  "$unapproved_url_installer" >/dev/null 2>&1; then
  fail "the candidate transport served an unapproved URL"
fi
[[ ! -e "$unapproved_url_output/unapproved" ]] ||
  fail "the candidate transport wrote bytes for an unapproved URL"

windows_asset="$distribution/heyfood-v0.6.2-x86_64-pc-windows-msvc.zip"
touch "$windows_asset"
if "$ROOT/scripts/release/checksums.sh" "$distribution" 0.6.2 >/dev/null 2>&1; then
  fail "checksum generation must reject a Windows v0.6.2 asset"
fi
if "$ROOT/scripts/release/verify-assets.sh" "$distribution" 0.6.2 >/dev/null 2>&1; then
  fail "complete-set verification must reject a Windows v0.6.2 asset"
fi

if grep -Eq \
  'cargo build --locked --release|scripts/release/package(-installer)?\.sh|packaging/macos/sign-and-notarize\.sh|scripts/release/checksums\.sh|actions/download-artifact' \
  "$RELEASE_WORKFLOW"; then
  fail "the tag workflow must not rebuild, re-sign, repackage, regenerate, or download same-run assets"
fi
grep -Fq 'environment: native-release' "$RELEASE_WORKFLOW" ||
  fail "publication must read approval bindings from the protected environment"
grep -Fq 'actions: read' "$RELEASE_WORKFLOW" ||
  fail "publication needs read-only access to the explicitly approved workflow artifact"
grep -Fq 'APPROVED_RUN_ID: ${{ vars.HEYFOOD_APPROVED_CANDIDATE_RUN_ID }}' \
  "$RELEASE_WORKFLOW" ||
  fail "publication must bind the approved protected run ID"
grep -Fq 'APPROVED_SHA256SUMS_SHA256: ${{ vars.HEYFOOD_APPROVED_CANDIDATE_SHA256SUMS_SHA256 }}' \
  "$RELEASE_WORKFLOW" ||
  fail "publication must bind the approved release-set digest"
for required in \
  '.event == "workflow_dispatch"' \
  '.status == "completed"' \
  '.conclusion == "success"' \
  '.head_sha == $commit' \
  '.path == ".github/workflows/ci.yml"' \
  '.name == "Native CLI CI"' \
  '.total_count == 1' \
  '.artifacts[0].expired == false' \
  '.artifacts[0].workflow_run.head_sha == $commit'; do
  grep -Fq "$required" "$RELEASE_WORKFLOW" ||
    fail "publication must fail closed on approved-run condition: $required"
done
grep -Fq 'artifacts?name=protected-candidate-release-set&per_page=100' \
  "$RELEASE_WORKFLOW" ||
  fail "publication must query only the aggregate protected artifact"
grep -Fq 'gh run download "$APPROVED_RUN_ID"' "$RELEASE_WORKFLOW" ||
  fail "publication must download from the explicitly approved run"
grep -Fq -- '--name protected-candidate-release-set' "$RELEASE_WORKFLOW" ||
  fail "publication must download only the aggregate protected release set"
grep -Fq 'test "$observed_manifest_sha256" = "$APPROVED_SHA256SUMS_SHA256"' \
  "$RELEASE_WORKFLOW" ||
  fail "publication must compare the exact approved SHA256SUMS digest"
grep -Fq 'scripts/release/verify-assets.sh dist "$VERSION" --native-state' \
  "$RELEASE_WORKFLOW" ||
  fail "publication must reverify the immutable ten-file release set"
grep -Fq 'gh attestation verify "$asset"' "$RELEASE_WORKFLOW" ||
  fail "publication must verify every protected attestation before publishing"
grep -Fq 'dist/*.json' "$RELEASE_WORKFLOW" ||
  fail "the release workflow must attest the native-state declaration"
grep -Fq 'dist/SHA256SUMS' "$RELEASE_WORKFLOW" ||
  fail "the release workflow must attest the checksum manifest"
metadata_line=$(line_of "$RELEASE_WORKFLOW" 'actions/runs/$APPROVED_RUN_ID"')
artifact_line=$(line_of "$RELEASE_WORKFLOW" 'artifacts?name=protected-candidate-release-set')
download_line=$(line_of "$RELEASE_WORKFLOW" 'gh run download "$APPROVED_RUN_ID"')
digest_line=$(line_of "$RELEASE_WORKFLOW" 'observed_manifest_sha256=')
verify_line=$(line_of "$RELEASE_WORKFLOW" 'scripts/release/verify-assets.sh')
protected_attestation_line=$(line_of "$RELEASE_WORKFLOW" 'gh attestation verify "$asset"')
publish_line=$(line_of "$RELEASE_WORKFLOW" 'gh release create')
if [[ -z "$metadata_line" || -z "$artifact_line" || -z "$download_line" ||
  -z "$digest_line" || -z "$verify_line" || -z "$protected_attestation_line" ||
  -z "$publish_line" ]] ||
  ! ((metadata_line < artifact_line &&
    artifact_line < download_line &&
    download_line < digest_line &&
    digest_line < verify_line &&
    verify_line < protected_attestation_line &&
    protected_attestation_line < publish_line)); then
  fail "publication must validate run, artifact, digest, set, and attestations before release creation"
fi
grep -Fq 'test "${#assets[@]}" -eq 10' "$PUBLIC_SMOKE_WORKFLOW" ||
  fail "public smoke must require all ten v0.8.0 assets"
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
grep -Fq 'installed_archive_core_release_matrix -- --ignored --exact' \
  "$CANDIDATE_WORKFLOW" ||
  fail "installed release qualification must execute the exact archive-driven matrix"
grep -Fq '"id": "household-contract-cli"' \
  "$ROOT/crates/heyfood-bin/tests/installed_showcase.rs" ||
  fail "installed release evidence must include the read-only household contract canary"
grep -Fq '"id": "selected-member-household-menu-cli"' \
  "$ROOT/crates/heyfood-bin/tests/installed_showcase.rs" ||
  fail "installed release evidence must cover the deployed selected-member agent envelope"
grep -Fq '"household_tui_lifecycle_not_automated"' \
  "$ROOT/crates/heyfood-bin/tests/installed_showcase.rs" ||
  fail "installed automation must preserve the manual household TUI boundary"

grep -Fq "Windows distribution remains deferred" "$ROOT/README.md" ||
  fail "README must state the Windows release boundary"
grep -Fq "Windows distribution and Windows CI are outside the v0.8.0 release train" "$ROOT/docs/CAPABILITY_STATUS.md" ||
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
  .distribution.ordinary_windows_ci_required == false and
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
    "member_profile_sync",
    "learned_dietary_graph",
    "member_health_and_fitness_data",
    "cross_device_household_state",
    "remote_member_erasure"
  ] and
  .manual_release_gates == [
    "clean_\($current)_install",
    "\($previous)_to_\($current)_upgrade",
    "current_\($current)_installer_refuses_\($previous)_request",
    "authorization_rollover_preserves_household_binding",
    "rotated_session_logout_refreshes_resumes_teardown_removes_vault_key_and_preserves_global_floor"
  ]
' --arg current "$CURRENT_GATE_VERSION" \
  --arg previous "$PREVIOUS_GATE_VERSION" \
  "$ROOT/tests/showcase/core-release-matrix.v1.json" >/dev/null ||
  fail "the core matrix must preserve the bounded distribution and non-gates"

printf 'release scope contract: public v%s; candidate v%s; macOS/Linux CI only\n' \
  "$SUPPORTED_RELEASE_VERSION" "$WORKSPACE_VERSION"
