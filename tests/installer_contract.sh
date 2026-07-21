#!/usr/bin/env bash

set -euo pipefail
IFS=$'\n\t'

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT
readonly INSTALLER="$ROOT/install.sh"
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/heyfood-installer-tests.XXXXXX")
readonly TEST_ROOT

cleanup() {
  rm -rf -- "$TEST_ROOT"
}
trap cleanup EXIT HUP INT TERM

fail() {
  printf 'installer contract test: %s\n' "$*" >&2
  exit 1
}

assert_contains() {
  local path="$1"
  local expected="$2"
  grep -F -- "$expected" "$path" >/dev/null ||
    fail "expected $path to contain: $expected"
}

assert_not_contains() {
  local path="$1"
  local unexpected="$2"
  if grep -Fi -- "$unexpected" "$path" >/dev/null; then
    fail "expected $path not to contain: $unexpected"
  fi
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{ print $1 }'
  else
    shasum -a 256 "$1" | awk '{ print $1 }'
  fi
}

host_target() {
  local target_os target_arch
  case "$(uname -s)" in
    Darwin) target_os="apple-darwin" ;;
    Linux) target_os="unknown-linux-gnu" ;;
    *) fail "tests require macOS or Linux" ;;
  esac
  case "$(uname -m)" in
    arm64 | aarch64) target_arch="aarch64" ;;
    x86_64 | amd64) target_arch="x86_64" ;;
    *) fail "tests require an aarch64 or x86_64 host" ;;
  esac
  printf '%s-%s\n' "$target_arch" "$target_os"
}

new_case() {
  local name="$1"
  CASE_DIR="$TEST_ROOT/$name"
  HOME_DIR="$CASE_DIR/home"
  BIN_DIR="$HOME_DIR/.local/bin"
  ASSET_DIR="$CASE_DIR/assets"
  FAKE_BIN="$CASE_DIR/fake-bin"
  DOWNLOAD_LOG="$CASE_DIR/downloads.log"
  STDOUT_LOG="$CASE_DIR/stdout.log"
  STDERR_LOG="$CASE_DIR/stderr.log"
  mkdir -p "$HOME_DIR" "$ASSET_DIR" "$FAKE_BIN"

  cat > "$FAKE_BIN/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

output=""
url=""
write_out=""
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --output)
      output="$2"
      shift 2
      ;;
    --write-out)
      write_out="$2"
      shift 2
      ;;
    --proto | --retry)
      shift 2
      ;;
    -qfsSL | --tlsv1.2)
      shift
      ;;
    https://*)
      url="$1"
      shift
      ;;
    *)
      printf 'unexpected fake curl argument: %s\n' "$1" >&2
      exit 64
      ;;
  esac
done

printf '%s\n' "$url" >> "$HEYFOOD_TEST_DOWNLOAD_LOG"
if [[ "$url" == */releases/latest ]]; then
  [[ "$write_out" == '%{url_effective}' ]] || exit 65
  printf 'https://github.com/frntrllc/heyfood/releases/tag/v%s' \
    "$HEYFOOD_TEST_LATEST_VERSION"
  exit 0
fi

[[ -n "$output" ]] || exit 66
cp "$HEYFOOD_TEST_ASSET_DIR/${url##*/}" "$output"
EOF
  chmod 0755 "$FAKE_BIN/curl"

  export CASE_DIR HOME_DIR BIN_DIR ASSET_DIR FAKE_BIN DOWNLOAD_LOG STDOUT_LOG STDERR_LOG
  export HOME="$HOME_DIR"
  export PATH="$FAKE_BIN:$PATH"
  export HEYFOOD_TEST_ASSET_DIR="$ASSET_DIR"
  export HEYFOOD_TEST_DOWNLOAD_LOG="$DOWNLOAD_LOG"
  export HEYFOOD_TEST_LATEST_VERSION="0.4.0"
  unset HEYFOOD_VERSION HEYFOOD_BIN_DIR XDG_BIN_HOME
}

make_release() {
  local version="$1"
  local reported_version="${2:-$version}"
  local target archive payload
  target=$(host_target)
  archive="heyfood-v$version-$target.tar.gz"
  payload="$CASE_DIR/payload"
  mkdir -p "$payload"
  cat > "$payload/heyfood" <<EOF
#!/usr/bin/env bash
if [[ "\${1:-}" == "--version" ]]; then
  printf 'heyfood %s\n' '$reported_version'
  exit 0
fi
exit 64
EOF
  chmod 0755 "$payload/heyfood"
  tar -czf "$ASSET_DIR/$archive" -C "$payload" heyfood
  printf '%s  %s\n' "$(sha256_file "$ASSET_DIR/$archive")" "$archive" \
    > "$ASSET_DIR/SHA256SUMS"
}

run_installer() {
  /bin/bash "$INSTALLER" >"$STDOUT_LOG" 2>"$STDERR_LOG"
}

test_source_invariants() {
  /bin/bash -n "$INSTALLER"
  [[ "$(sha256_file "$INSTALLER")  install.sh" == "$(tr -d '\n' < "$ROOT/install.sh.sha256")" ]] ||
    fail "install.sh.sha256 does not match install.sh"
  assert_contains "$INSTALLER" 'set -euo pipefail'
  assert_contains "$INSTALLER" 'https://github.com'
  assert_contains "$INSTALLER" 'SHA256SUMS'
  assert_contains "$INSTALLER" "mv -f -- \"\$STAGED_EXECUTABLE\" \"\$INSTALL_PATH\""
  assert_not_contains "$INSTALLER" 'pypi'
  assert_not_contains "$INSTALLER" 'pipx'
  assert_not_contains "$INSTALLER" 'python'
  if grep -E '^[[:space:]]*(sudo|eval)([[:space:]]|$)' "$INSTALLER" >/dev/null; then
    fail "installer must not invoke sudo or eval"
  fi
  for startup in .bashrc .zshrc .profile; do
    assert_not_contains "$INSTALLER" "$startup"
  done
}

test_exact_native_install() {
  new_case exact
  make_release 0.4.0
  export HEYFOOD_VERSION=0.4.0
  run_installer

  [[ -x "$BIN_DIR/heyfood" ]] || fail "native executable was not installed"
  [[ "$("$BIN_DIR/heyfood" --version)" == "heyfood 0.4.0" ]] ||
    fail "installed executable did not report the exact release version"
  assert_contains "$STDOUT_LOG" "Installed heyfood 0.4.0 at $BIN_DIR/heyfood"
  assert_contains "$STDOUT_LOG" "Connect an account: heyfood register"
  assert_not_contains "$STDOUT_LOG" "heyfood login"
  assert_contains "$DOWNLOAD_LOG" "/releases/download/v0.4.0/SHA256SUMS"
  assert_contains "$DOWNLOAD_LOG" "/releases/download/v0.4.0/heyfood-v0.4.0-$(host_target).tar.gz"
}

test_latest_release_resolution() {
  new_case latest
  make_release 0.4.0
  run_installer

  [[ "$("$BIN_DIR/heyfood" --version)" == "heyfood 0.4.0" ]] ||
    fail "latest release was not installed"
  assert_contains "$DOWNLOAD_LOG" "/releases/latest"
}

test_streamed_install() {
  new_case streamed
  make_release 0.4.0
  export HEYFOOD_VERSION=0.4.0
  /bin/bash < "$INSTALLER" >"$STDOUT_LOG" 2>"$STDERR_LOG"
  [[ -x "$BIN_DIR/heyfood" ]] || fail "streamed installer did not install heyfood"
}

test_rejects_version_injection_before_download() {
  new_case version-injection
  export HEYFOOD_VERSION="0.4.0;touch-must-not-run"
  if run_installer; then
    fail "installer accepted an unsafe version"
  fi
  [[ ! -e "$DOWNLOAD_LOG" ]] || fail "installer downloaded before rejecting the version"
  assert_contains "$STDERR_LOG" "HEYFOOD_VERSION must be an exact release"
}

test_rejects_uncontrolled_install_targets() {
  new_case relative-bin
  make_release 0.4.0
  export HEYFOOD_VERSION=0.4.0 HEYFOOD_BIN_DIR="relative/bin"
  if run_installer; then
    fail "installer accepted a relative bin directory"
  fi
  [[ ! -e "$DOWNLOAD_LOG" ]] || fail "installer downloaded before rejecting the directory"

  new_case symlink-bin
  make_release 0.4.0
  mkdir -p "$CASE_DIR/real-bin"
  ln -s "$CASE_DIR/real-bin" "$CASE_DIR/bin-link"
  export HEYFOOD_VERSION=0.4.0 HEYFOOD_BIN_DIR="$CASE_DIR/bin-link"
  if run_installer; then
    fail "installer accepted a symlink bin directory"
  fi
  [[ ! -e "$DOWNLOAD_LOG" ]] || fail "installer downloaded before rejecting the symlink"

  new_case shared-bin
  make_release 0.4.0
  mkdir -p "$CASE_DIR/shared-bin"
  chmod 0775 "$CASE_DIR/shared-bin"
  export HEYFOOD_VERSION=0.4.0 HEYFOOD_BIN_DIR="$CASE_DIR/shared-bin"
  if run_installer; then
    fail "installer accepted a group-writable bin directory"
  fi
  [[ ! -e "$DOWNLOAD_LOG" ]] || fail "installer downloaded into a shared directory"

  new_case symlink-target
  make_release 0.4.0
  mkdir -p "$BIN_DIR"
  printf 'do not replace\n' > "$CASE_DIR/target"
  ln -s "$CASE_DIR/target" "$BIN_DIR/heyfood"
  export HEYFOOD_VERSION=0.4.0
  if run_installer; then
    fail "installer accepted a symlink executable target"
  fi
  [[ ! -e "$DOWNLOAD_LOG" ]] || fail "installer downloaded before rejecting the target"
  [[ "$(cat "$CASE_DIR/target")" == "do not replace" ]] ||
    fail "installer followed the existing executable symlink"
}

write_existing_binary() {
  mkdir -p "$BIN_DIR"
  cat > "$BIN_DIR/heyfood" <<'EOF'
#!/usr/bin/env bash
printf 'prior complete executable\n'
EOF
  chmod 0755 "$BIN_DIR/heyfood"
}

assert_existing_binary_untouched() {
  [[ "$("$BIN_DIR/heyfood")" == "prior complete executable" ]] ||
    fail "a failed installation changed the existing executable"
}

test_checksum_failure_preserves_existing_binary() {
  new_case bad-checksum
  make_release 0.4.0
  write_existing_binary
  printf 'corruption\n' >> "$ASSET_DIR/heyfood-v0.4.0-$(host_target).tar.gz"
  export HEYFOOD_VERSION=0.4.0
  if run_installer; then
    fail "installer accepted an invalid checksum"
  fi
  assert_contains "$STDERR_LOG" "checksum verification failed"
  assert_existing_binary_untouched
}

test_archive_shape_failure_preserves_existing_binary() {
  new_case bad-archive
  make_release 0.4.0
  write_existing_binary
  printf 'unexpected\n' > "$CASE_DIR/payload/unexpected.txt"
  tar -czf "$ASSET_DIR/heyfood-v0.4.0-$(host_target).tar.gz" \
    -C "$CASE_DIR/payload" heyfood unexpected.txt
  printf '%s  %s\n' \
    "$(sha256_file "$ASSET_DIR/heyfood-v0.4.0-$(host_target).tar.gz")" \
    "heyfood-v0.4.0-$(host_target).tar.gz" > "$ASSET_DIR/SHA256SUMS"
  export HEYFOOD_VERSION=0.4.0
  if run_installer; then
    fail "installer accepted unexpected archive members"
  fi
  assert_contains "$STDERR_LOG" "must contain only the heyfood executable"
  assert_existing_binary_untouched
}

test_version_mismatch_preserves_existing_binary() {
  new_case wrong-version
  make_release 0.4.0 9.9.9
  write_existing_binary
  export HEYFOOD_VERSION=0.4.0
  if run_installer; then
    fail "installer accepted an executable with the wrong version"
  fi
  assert_contains "$STDERR_LOG" "expected heyfood 0.4.0 before installation"
  assert_existing_binary_untouched
}

test_source_invariants
test_exact_native_install
test_latest_release_resolution
test_streamed_install
test_rejects_version_injection_before_download
test_rejects_uncontrolled_install_targets
test_checksum_failure_preserves_existing_binary
test_archive_shape_failure_preserves_existing_binary
test_version_mismatch_preserves_existing_binary

printf 'installer contract: all tests passed for %s\n' "$(host_target)"
