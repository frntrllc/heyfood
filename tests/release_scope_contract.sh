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

protected_slice="$CASE_DIR/protected-ci.yml"
sed -n '/^  protected-candidate-preflight:/,$p' "$CANDIDATE_WORKFLOW" >"$protected_slice"

[[ -x "$ROOT/packaging/macos/sign-and-notarize.sh" ]] ||
  fail "the macOS signing tool must be executable"
[[ "$(git -C "$ROOT" ls-files --stage -- packaging/macos/sign-and-notarize.sh |
  awk '{print $1}')" == "100755" ]] ||
  fail "Git must record the macOS signing tool with mode 100755"

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
grep -Fq 'Test the default product feature set on macOS and Windows' \
  "$ROOT/.github/workflows/rust-ci.yml" ||
  fail "ordinary Rust Windows tests must remain enabled"

distribution="$CASE_DIR/distribution"
mkdir "$distribution"
for target in \
  aarch64-apple-darwin \
  aarch64-unknown-linux-gnu \
  x86_64-apple-darwin \
  x86_64-unknown-linux-gnu; do
  "$ROOT/scripts/release/package.sh" "$ROOT/install.sh" 0.5.0 "$target" "$distribution"
done
"$ROOT/scripts/release/checksums.sh" "$distribution" 0.5.0
"$ROOT/scripts/release/verify-assets.sh" "$distribution" 0.5.0
[[ "$(wc -l <"$distribution/SHA256SUMS" | tr -d '[:space:]')" -eq 4 ]] ||
  fail "the release manifest must bind exactly four archives"

windows_asset="$distribution/heyfood-v0.5.0-x86_64-pc-windows-msvc.zip"
touch "$windows_asset"
if "$ROOT/scripts/release/checksums.sh" "$distribution" 0.5.0 >/dev/null 2>&1; then
  fail "checksum generation must reject a Windows v0.5.0 asset"
fi
if "$ROOT/scripts/release/verify-assets.sh" "$distribution" 0.5.0 >/dev/null 2>&1; then
  fail "complete-set verification must reject a Windows v0.5.0 asset"
fi

grep -Fq "Windows distribution is deferred to \`v0.5.1\`" "$ROOT/README.md" ||
  fail "README must state the Windows release boundary"
grep -Fq "Windows distribution is deferred to \`v0.5.1\`" "$ROOT/docs/CAPABILITY_STATUS.md" ||
  fail "capability status must state the Windows release boundary"
grep -Fq "Windows distribution is deferred to \`v0.5.1\`" "$ROOT/docs/RELEASE_SIGNING.md" ||
  fail "signing policy must state the Windows release boundary"
jq -e '
  .distribution.release_targets == [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu"
  ] and
  .distribution.windows_distribution == "deferred_to_0.5.1" and
  .distribution.ordinary_windows_ci_required == true and
  .explicit_non_gates == [
    "native_voice",
    "menu_watch_diff",
    "health_integrations"
  ]
' "$ROOT/tests/showcase/core-release-matrix.v1.json" >/dev/null ||
  fail "the core matrix must preserve the bounded distribution and non-gates"

printf 'release scope contract: four v0.5.0 archives; Windows CI retained\n'
