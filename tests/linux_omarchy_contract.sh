#!/usr/bin/env bash

set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT
readonly BASELINE="$ROOT/docs/release-evidence/linux-omarchy/phase0-baseline.json"

fail() {
  printf 'Linux/Omarchy contract: %s\n' "$*" >&2
  exit 1
}

jq -e '
  .schema_version == 1 and
  .source_baseline.version == "0.9.0" and
  .public_release_baseline.immutable == true and
  .locked_boundaries.linux_targets == [
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu"
  ] and
  .locked_boundaries.maximum_glibc == "2.34" and
  .locked_boundaries.authentication == "device_authorization" and
  .locked_boundaries.native_credentials == "secret_service_fail_closed" and
  .locked_boundaries.file_credentials == "explicit_opt_in_only" and
  .locked_boundaries.aur_package == "heyfood-bin" and
  .existing_work.native_linux_builds == true and
  .existing_work.hosted_linux_installer == true and
  (
    (.phase0_complete == false and
      (.phase0_blocker | type == "string" and length > 0)) or
    (.phase0_complete == true and
      (.ownership.aur_maintainer | type == "string" and length > 0) and
      (.ownership.aur_co_maintainer | type == "string" and length > 0) and
      (.ownership.protected_environment_owner | type == "string" and length > 0) and
      (.ownership.incident_owner | type == "string" and length > 0))
  )
' "$BASELINE" >/dev/null || fail "the Phase 0 baseline is incomplete or overstated"

for helper in scripts/release/native-state-declaration.sh scripts/release/verify-linux-elf.sh; do
  [[ -x "$ROOT/$helper" ]] || fail "$helper must be executable"
  [[ "$(git -C "$ROOT" ls-files --stage -- "$helper" | awk '{print $1}')" == "100755" ]] ||
    fail "Git must record $helper with mode 100755"
done

grep -Fq 'scripts/release/verify-linux-elf.sh' \
  "$ROOT/scripts/release/smoke-archive.sh" ||
  fail "installed Linux archives must pass the ELF gate"
grep -Fq 'Record the protected Linux ELF boundary' "$ROOT/.github/workflows/ci.yml" ||
  fail "protected Linux candidates must retain ELF evidence"
grep -Fq 'Install Linux ELF inspection tooling' \
  "$ROOT/.github/workflows/post-release-smoke.yml" ||
  fail "public Linux smoke must install its ELF inspection dependency"

if grep -Eq '^pub const (ACCENT|CHROME|FOCUS|SELECTED|USER|NOTICE): Color = Color::Rgb' \
  "$ROOT/crates/heyfood-tui/src/theme.rs"; then
  fail "terminal-owned semantic roles must not pin RGB values"
fi

grep -Fq '| Linux | glibc 2.34 |' \
  "$ROOT/docs/release-evidence/rust-phase0/dependency-platform-trust-inventory.md" ||
  fail "the supported Linux runtime floor must be glibc 2.34"
