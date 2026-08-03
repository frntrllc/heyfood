#!/usr/bin/env bash

set -euo pipefail
IFS=$'\n\t'

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT
readonly INSTALLER="$ROOT/install.sh"
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/heyfood-installer-tests.XXXXXX")
readonly TEST_ROOT

(
  cd "$ROOT"
  cargo build --quiet --locked --package heyfood-installer
)
TARGET_DIRECTORY=$(
  cd "$ROOT"
  cargo metadata --quiet --locked --no-deps --format-version 1 |
    jq -er '.target_directory'
)
readonly TARGET_DIRECTORY
readonly VERIFIER_BINARY="$TARGET_DIRECTORY/debug/heyfood-installer"
test -x "$VERIFIER_BINARY"

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

line_of_exact() {
  local path="$1"
  local expected="$2"
  awk -v expected="$expected" '$0 == expected { print NR; exit }' "$path"
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{ print $1 }'
  else
    shasum -a 256 "$1" | awk '{ print $1 }'
  fi
}

sha256_stdin() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum | awk '{ print $1 }'
  else
    shasum -a 256 | awk '{ print $1 }'
  fi
}

byte_length() {
  LC_ALL=C printf '%s' "$1" | wc -c | tr -d '[:space:]'
}

write_u32_be() {
  local value="$1"
  local encoded
  printf -v encoded '\\%03o\\%03o\\%03o\\%03o' \
    "$(((value >> 24) & 255))" \
    "$(((value >> 16) & 255))" \
    "$(((value >> 8) & 255))" \
    "$((value & 255))"
  printf '%b' "$encoded"
}

native_root_instance_digest() {
  local platform_label="$1"
  local physical_root="$2"
  local platform_length root_length
  platform_length=$(byte_length "$platform_label")
  root_length=$(byte_length "$physical_root")
  {
    printf '%s\0' 'heyfood.household.native-root-instance.v1'
    write_u32_be "$platform_length"
    printf '%s' "$platform_label"
    write_u32_be "$root_length"
    printf '%s' "$physical_root"
  } | sha256_stdin
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
  CASE_INSTALLER="$INSTALLER"
  mkdir -p "$HOME_DIR" "$ASSET_DIR" "$FAKE_BIN"

  cat >"$FAKE_BIN/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

output=""
url=""
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --output)
      output="$2"
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

printf '%s\n' "$url" >>"$HEYFOOD_TEST_DOWNLOAD_LOG"
[[ -n "$output" ]] || exit 66
cp "$HEYFOOD_TEST_ASSET_DIR/${url##*/}" "$output"
EOF
  chmod 0755 "$FAKE_BIN/curl"

  export CASE_DIR HOME_DIR BIN_DIR ASSET_DIR FAKE_BIN DOWNLOAD_LOG STDOUT_LOG STDERR_LOG
  export HOME="$HOME_DIR"
  export PATH="$FAKE_BIN:$PATH"
  export HEYFOOD_TEST_ASSET_DIR="$ASSET_DIR"
  export HEYFOOD_TEST_DOWNLOAD_LOG="$DOWNLOAD_LOG"
  unset HEYFOOD_VERSION HEYFOOD_BIN_DIR HEYFOOD_STATE_DIR XDG_BIN_HOME XDG_DATA_HOME
}

use_native_state_installer() {
  CASE_INSTALLER="$INSTALLER"
}

make_release() {
  local version="$1"
  local reported_version="${2:-$version}"
  local mode="${3:-legacy}"
  local manifest_variant="${4:-exact}"
  local candidate_native_version="${5:-2}"
  local release_native_version="${6:-$candidate_native_version}"
  local verifier_mode="${7:-real}"
  local target archive declaration verifier_archive payload manifest_fixture
  target=$(host_target)
  archive="heyfood-v$version-$target.tar.gz"
  declaration="heyfood-v$version-native-state.json"
  verifier_archive="heyfood-installer-v$version-$target.tar.gz"
  payload="$CASE_DIR/payload"
  mkdir -p "$payload"
  manifest_fixture="$CASE_DIR/candidate-manifest.json"

  if [[ "$mode" == "native" ]]; then
    case "$manifest_variant" in
      exact)
        printf '%s' \
          "{\"binary_version\":\"$version\",\"native_state_compatibility\":{\"binary_version\":\"$version\",\"maximum_native_state_version\":$candidate_native_version,\"native_state_capabilities\":[\"household-account-slot-v1\",\"household-lifecycle-lock-v1\",\"household-migration-guard-v1\",\"household-teardown-journal-v1\"],\"schema_version\":1},\"schema_version\":2}" \
          >"$manifest_fixture"
        ;;
      nested)
        printf '%s' \
          "{\"native\":{\"native_state_compatibility\":{\"binary_version\":\"$version\",\"maximum_native_state_version\":$candidate_native_version,\"native_state_capabilities\":[\"household-account-slot-v1\",\"household-lifecycle-lock-v1\",\"household-migration-guard-v1\",\"household-teardown-journal-v1\"],\"schema_version\":1}},\"schema_version\":2}" \
          >"$manifest_fixture"
        ;;
      textual)
        printf '%s' \
          "{\"description\":\"native_state_compatibility: {\\\"binary_version\\\":\\\"$version\\\"}\",\"schema_version\":2}" \
          >"$manifest_fixture"
        ;;
      duplicate-top)
        printf '%s' \
          "{\"native_state_compatibility\":{\"binary_version\":\"$version\",\"maximum_native_state_version\":2,\"native_state_capabilities\":[\"household-account-slot-v1\",\"household-lifecycle-lock-v1\",\"household-migration-guard-v1\",\"household-teardown-journal-v1\"],\"schema_version\":1},\"native_state_compatibility\":{\"binary_version\":\"$version\",\"maximum_native_state_version\":2,\"native_state_capabilities\":[\"household-account-slot-v1\",\"household-lifecycle-lock-v1\",\"household-migration-guard-v1\",\"household-teardown-journal-v1\"],\"schema_version\":1},\"schema_version\":2}" \
          >"$manifest_fixture"
        ;;
      duplicate-nested)
        printf '%s' \
          "{\"native_state_compatibility\":{\"binary_version\":\"$version\",\"maximum_native_state_version\":2,\"native_state_capabilities\":[\"household-account-slot-v1\",\"household-lifecycle-lock-v1\",\"household-migration-guard-v1\",\"household-teardown-journal-v1\"],\"schema_version\":1,\"schema_version\":1},\"schema_version\":2}" \
          >"$manifest_fixture"
        ;;
      wrong-schema)
        printf '%s' \
          "{\"binary_version\":\"$version\",\"native_state_compatibility\":{\"binary_version\":\"$version\",\"maximum_native_state_version\":2,\"native_state_capabilities\":[\"household-account-slot-v1\",\"household-lifecycle-lock-v1\",\"household-migration-guard-v1\",\"household-teardown-journal-v1\"],\"schema_version\":1},\"schema_version\":1}" \
          >"$manifest_fixture"
        ;;
      invalid)
        printf '%s' '{"native_state_compatibility":' >"$manifest_fixture"
        ;;
      oversized)
        {
          printf '%s' '{"padding":"'
          head -c 1048576 /dev/zero | tr '\0' x
          printf '%s' '"}'
        } >"$manifest_fixture"
        ;;
      *)
        fail "unknown manifest fixture variant: $manifest_variant"
        ;;
    esac
  fi

  cat >"$payload/heyfood" <<EOF
#!/usr/bin/env bash
if [[ "\${1:-}" == "--version" ]]; then
  printf 'heyfood %s\n' '$reported_version'
  exit 0
fi
if [[ "\$#" == "4" && "\${1:-}" == "agent" && "\${2:-}" == "describe" && \
  "\${3:-}" == "--schema-version" && "\${4:-}" == "2" && "$mode" == "native" ]]; then
  cat '$manifest_fixture'
  printf '\n'
  exit 0
fi
exit 64
EOF
  chmod 0755 "$payload/heyfood"
  tar -czf "$ASSET_DIR/$archive" -C "$payload" heyfood
  if [[ "$mode" == "native" ]]; then
    printf '%s' \
      "{\"binary_version\":\"$version\",\"maximum_native_state_version\":$release_native_version,\"native_state_capabilities\":[\"household-account-slot-v1\",\"household-lifecycle-lock-v1\",\"household-migration-guard-v1\",\"household-teardown-journal-v1\"],\"schema_version\":1}" \
      >"$ASSET_DIR/$declaration"
    mkdir -p "$CASE_DIR/verifier-payload"
    case "$verifier_mode" in
      real)
        cp "$VERIFIER_BINARY" "$CASE_DIR/verifier-payload/heyfood-installer"
        ;;
      reject)
        cat >"$CASE_DIR/verifier-payload/heyfood-installer" <<EOF
#!/usr/bin/env bash
if [[ "\${1:-}" == "--version" ]]; then
  printf 'heyfood-installer %s\n' '$version'
  exit 0
fi
exit 1
EOF
        ;;
      interrupt)
        cat >"$CASE_DIR/verifier-payload/heyfood-installer" <<EOF
#!/usr/bin/env bash
if [[ "\${1:-}" == "--version" ]]; then
  printf 'heyfood-installer %s\n' '$version'
  exit 0
fi
kill -TERM "\$PPID"
exit 130
EOF
        ;;
      *)
        fail "unknown verifier mode: $verifier_mode"
        ;;
    esac
    chmod 0755 "$CASE_DIR/verifier-payload/heyfood-installer"
    tar -czf "$ASSET_DIR/$verifier_archive" \
      -C "$CASE_DIR/verifier-payload" \
      heyfood-installer
    {
      printf '%s  %s\n' "$(sha256_file "$ASSET_DIR/$archive")" "$archive"
      printf '%s  %s\n' "$(sha256_file "$ASSET_DIR/$verifier_archive")" "$verifier_archive"
      printf '%s  %s\n' "$(sha256_file "$ASSET_DIR/$declaration")" "$declaration"
    } >"$ASSET_DIR/SHA256SUMS"
  else
    printf '%s  %s\n' "$(sha256_file "$ASSET_DIR/$archive")" "$archive" \
      >"$ASSET_DIR/SHA256SUMS"
  fi
}

refresh_native_checksums() {
  local version="$1"
  local target archive verifier_archive declaration
  target=$(host_target)
  archive="heyfood-v$version-$target.tar.gz"
  verifier_archive="heyfood-installer-v$version-$target.tar.gz"
  declaration="heyfood-v$version-native-state.json"
  {
    printf '%s  %s\n' "$(sha256_file "$ASSET_DIR/$archive")" "$archive"
    printf '%s  %s\n' "$(sha256_file "$ASSET_DIR/$verifier_archive")" "$verifier_archive"
    printf '%s  %s\n' "$(sha256_file "$ASSET_DIR/$declaration")" "$declaration"
  } >"$ASSET_DIR/SHA256SUMS"
}

run_installer() {
  /bin/bash "$CASE_INSTALLER" >"$STDOUT_LOG" 2>"$STDERR_LOG"
}

write_native_state_floor() {
  local native_root="$CASE_DIR/native-state/data"
  local platform_label root_digest

  export HEYFOOD_STATE_DIR="$CASE_DIR/native-state"
  mkdir -p "$native_root/compatibility"
  chmod 0700 "$CASE_DIR/native-state" "$native_root" "$native_root/compatibility"
  case "$(uname -s)" in
    Darwin) platform_label="macos" ;;
    Linux) platform_label="linux" ;;
    *) fail "tests require macOS or Linux" ;;
  esac
  root_digest=$(native_root_instance_digest "$platform_label" "$(cd "$native_root" && pwd -P)")
  printf '%s' \
    "{\"floor_revision\":1,\"minimum_compatible_native_state_version\":2,\"native_root_instance_digest\":\"$root_digest\",\"required_binary_capabilities\":[\"household-account-slot-v1\",\"household-lifecycle-lock-v1\",\"household-migration-guard-v1\",\"household-teardown-journal-v1\"],\"schema_version\":1}" \
    >"$native_root/compatibility/native-state-floor.v1.json"
  chmod 0600 "$native_root/compatibility/native-state-floor.v1.json"
}

test_source_invariants() {
  local download_line floor_line version_gate_line
  /bin/bash -n "$INSTALLER"
  [[ "$(sha256_file "$INSTALLER")  install.sh" == "$(tr -d '\n' <"$ROOT/install.sh.sha256")" ]] ||
    fail "install.sh.sha256 does not match install.sh"
  assert_contains "$INSTALLER" 'set -euo pipefail'
  assert_contains "$INSTALLER" 'https://github.com'
  assert_contains "$INSTALLER" 'SUPPORTED_VERSION="0.8.0"'
  assert_contains "$INSTALLER" 'NATIVE_STATE_RELEASE_VERSION="0.8.0"'
  assert_contains "$INSTALLER" '.local/pipx/venvs/heyfood-cli/bin/heyfood'
  assert_contains "$INSTALLER" 'SHA256SUMS'
  assert_contains "$INSTALLER" 'native-state.json'
  assert_contains "$INSTALLER" 'native-state-floor.v1.json'
  assert_contains "$INSTALLER" 'agent describe --schema-version 2'
  assert_contains "$INSTALLER" "\"\$STAGED_VERIFIER\" verify-native-state"
  assert_contains "$INSTALLER" "mv -f -- \"\$STAGED_EXECUTABLE\" \"\$INSTALL_PATH\""
  assert_not_contains "$INSTALLER" 'grep -o '"'"'"native_state_compatibility"'"'"
  assert_not_contains "$INSTALLER" "sed -n 's/^.*"
  assert_not_contains "$INSTALLER" 'HEYFOOD_NATIVE_INSTALLATION_SUSPENDED'
  assert_not_contains "$INSTALLER" 'pypi'
  assert_not_contains "$INSTALLER" 'python'
  version_gate_line=$(line_of_exact \
    "$INSTALLER" "[[ \"\$VERSION\" == \"\$SUPPORTED_VERSION\" ]] ||")
  floor_line=$(line_of_exact "$INSTALLER" 'inspect_native_state_floor')
  download_line=$(line_of_exact \
    "$INSTALLER" \
    "download \"\$DOWNLOAD_BASE/\$CHECKSUMS\" \"\$CHECKSUMS_PATH\" ||")
  if [[ -z "$version_gate_line" || -z "$floor_line" || -z "$download_line" ||
    "$version_gate_line" -ge "$floor_line" || "$floor_line" -ge "$download_line" ]]; then
    fail "the exact supported-version gate must precede floor inspection and downloads"
  fi
  if grep -E '^[[:space:]]*(sudo|eval)([[:space:]]|$)' "$INSTALLER" >/dev/null; then
    fail "installer must not invoke sudo or eval"
  fi
  for startup in .bashrc .zshrc .profile; do
    assert_not_contains "$INSTALLER" "$startup"
  done
}

test_native_root_digest_vectors() {
  [[ "$(native_root_instance_digest \
    macos \
    "/Users/alice/Library/Application Support/ai.frntr.heyfood")" == \
    "61c1a73e0f6dc4059111ba62a9c1f79bf06da4e65f90bcbc0da0cba6dab13a9a" ]] ||
    fail "macOS native-root digest drifted from the Rust contract"
  [[ "$(native_root_instance_digest \
    linux \
    "/home/alice/.local/share/heyfood")" == \
    "eca9baf8e73318a57e522116993dddf48f7dcc833b89d12123e4bd424ac39ad8" ]] ||
    fail "Linux native-root digest drifted from the Rust contract"
}

test_exact_native_install() {
  new_case exact
  make_release 0.8.0 0.8.0 native
  export HEYFOOD_VERSION=0.8.0
  run_installer

  [[ -x "$BIN_DIR/heyfood" ]] || fail "native executable was not installed"
  [[ "$("$BIN_DIR/heyfood" --version)" == "heyfood 0.8.0" ]] ||
    fail "installed executable did not report the exact release version"
  assert_contains "$STDOUT_LOG" "Installed heyfood 0.8.0 at $BIN_DIR/heyfood"
  assert_contains "$STDOUT_LOG" "Open heyfood to sign in or create an account: heyfood"
  assert_contains "$STDOUT_LOG" "Agent contract: heyfood agent describe"
  assert_contains "$STDOUT_LOG" "Optional Codex/Claude setup preview:"
  assert_contains "$DOWNLOAD_LOG" "/releases/download/v0.8.0/SHA256SUMS"
  assert_contains "$DOWNLOAD_LOG" "/releases/download/v0.8.0/heyfood-v0.8.0-$(host_target).tar.gz"
  assert_contains "$DOWNLOAD_LOG" "/releases/download/v0.8.0/heyfood-v0.8.0-native-state.json"
  assert_contains "$DOWNLOAD_LOG" "/releases/download/v0.8.0/heyfood-installer-v0.8.0-$(host_target).tar.gz"
}

test_default_release_is_current() {
  new_case default
  make_release 0.8.0 0.8.0 native
  run_installer

  [[ "$("$BIN_DIR/heyfood" --version)" == "heyfood 0.8.0" ]] ||
    fail "default supported release was not installed"
  assert_contains "$DOWNLOAD_LOG" "/releases/download/v0.8.0/"
  assert_not_contains "$DOWNLOAD_LOG" "/releases/latest"
}

test_streamed_install() {
  new_case streamed
  make_release 0.8.0 0.8.0 native
  /bin/bash <"$INSTALLER" >"$STDOUT_LOG" 2>"$STDERR_LOG"
  [[ -x "$BIN_DIR/heyfood" ]] || fail "streamed installer did not install heyfood"
}

test_rejects_unsupported_or_unsafe_version_before_download() {
  new_case unsupported-version
  export HEYFOOD_VERSION=0.4.1
  if run_installer; then
    fail "installer accepted an unsupported incident release"
  fi
  [[ ! -e "$DOWNLOAD_LOG" ]] || fail "installer downloaded before rejecting the unsupported release"
  assert_contains "$STDERR_LOG" "this installer supports heyfood 0.8.0"

  new_case version-injection
  export HEYFOOD_VERSION="0.8.0;touch-must-not-run"
  if run_installer; then
    fail "installer accepted an unsafe version"
  fi
  [[ ! -e "$DOWNLOAD_LOG" ]] || fail "installer downloaded before rejecting the version"
  assert_contains "$STDERR_LOG" "HEYFOOD_VERSION must be an exact release"
}

test_rejects_uncontrolled_install_targets() {
  new_case relative-bin
  make_release 0.8.0 0.8.0 native
  export HEYFOOD_BIN_DIR="relative/bin"
  if run_installer; then
    fail "installer accepted a relative bin directory"
  fi
  [[ ! -e "$DOWNLOAD_LOG" ]] || fail "installer downloaded before rejecting the directory"

  new_case symlink-bin
  make_release 0.8.0 0.8.0 native
  mkdir -p "$CASE_DIR/real-bin"
  ln -s "$CASE_DIR/real-bin" "$CASE_DIR/bin-link"
  export HEYFOOD_BIN_DIR="$CASE_DIR/bin-link"
  if run_installer; then
    fail "installer accepted a symlink bin directory"
  fi
  [[ ! -e "$DOWNLOAD_LOG" ]] || fail "installer downloaded before rejecting the symlink"

  new_case shared-bin
  make_release 0.8.0 0.8.0 native
  mkdir -p "$CASE_DIR/shared-bin"
  chmod 0775 "$CASE_DIR/shared-bin"
  export HEYFOOD_BIN_DIR="$CASE_DIR/shared-bin"
  if run_installer; then
    fail "installer accepted a group-writable bin directory"
  fi
  [[ ! -e "$DOWNLOAD_LOG" ]] || fail "installer downloaded into a shared directory"

  new_case symlink-target
  make_release 0.8.0 0.8.0 native
  mkdir -p "$BIN_DIR"
  printf 'do not replace\n' >"$CASE_DIR/target"
  ln -s "$CASE_DIR/target" "$BIN_DIR/heyfood"
  if run_installer; then
    fail "installer accepted a symlink executable target"
  fi
  [[ ! -e "$DOWNLOAD_LOG" ]] || fail "installer downloaded before rejecting the target"
  [[ "$(cat "$CASE_DIR/target")" == "do not replace" ]] ||
    fail "installer followed the existing executable symlink"
}

make_legacy_pipx_link() {
  local legacy_target="$HOME_DIR/.local/pipx/venvs/heyfood-cli/bin/heyfood"
  mkdir -p "$BIN_DIR" "$(dirname "$legacy_target")"
  cat >"$legacy_target" <<'EOF'
#!/usr/bin/env bash
printf 'heyfood 0.3.2\n'
EOF
  chmod 0755 "$legacy_target"
  ln -s "$legacy_target" "$BIN_DIR/heyfood"
}

test_migrates_known_legacy_pipx_symlink() {
  new_case legacy-pipx
  make_release 0.8.0 0.8.0 native
  make_legacy_pipx_link
  run_installer

  [[ -f "$BIN_DIR/heyfood" && ! -L "$BIN_DIR/heyfood" ]] ||
    fail "legacy pipx symlink was not replaced by the native executable"
  [[ "$("$BIN_DIR/heyfood" --version)" == "heyfood 0.8.0" ]] ||
    fail "legacy pipx migration did not install v0.8.0"
  [[ "$("$HOME_DIR/.local/pipx/venvs/heyfood-cli/bin/heyfood")" == "heyfood 0.3.2" ]] ||
    fail "legacy pipx migration changed the old environment"
}

test_failed_install_preserves_known_legacy_pipx_symlink() {
  new_case legacy-pipx-failure
  make_release 0.8.0 0.8.0 native
  make_legacy_pipx_link
  printf 'corruption\n' >>"$ASSET_DIR/heyfood-v0.8.0-$(host_target).tar.gz"
  if run_installer; then
    fail "legacy pipx migration accepted an invalid checksum"
  fi

  [[ -L "$BIN_DIR/heyfood" ]] ||
    fail "failed migration removed the legacy pipx symlink"
  [[ "$(readlink "$BIN_DIR/heyfood")" == "$HOME_DIR/.local/pipx/venvs/heyfood-cli/bin/heyfood" ]] ||
    fail "failed migration changed the legacy pipx symlink"
  [[ "$("$BIN_DIR/heyfood")" == "heyfood 0.3.2" ]] ||
    fail "failed migration changed the legacy pipx executable"
}

write_existing_binary() {
  mkdir -p "$BIN_DIR"
  cat >"$BIN_DIR/heyfood" <<'EOF'
#!/usr/bin/env bash
printf 'prior complete executable\n'
EOF
  chmod 0755 "$BIN_DIR/heyfood"
}

assert_existing_binary_untouched() {
  [[ "$("$BIN_DIR/heyfood")" == "prior complete executable" ]] ||
    fail "a failed installation changed the existing executable"
}

write_existing_current_binary() {
  mkdir -p "$BIN_DIR"
  cat >"$BIN_DIR/heyfood" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == "--version" ]]; then
  printf 'heyfood 0.8.0\n'
  exit 0
fi
printf 'installed v0.8.0 sentinel\n'
EOF
  chmod 0755 "$BIN_DIR/heyfood"
}

test_checksum_failure_preserves_existing_binary() {
  new_case bad-checksum
  make_release 0.8.0 0.8.0 native
  write_existing_binary
  printf 'corruption\n' >>"$ASSET_DIR/heyfood-v0.8.0-$(host_target).tar.gz"
  if run_installer; then
    fail "installer accepted an invalid checksum"
  fi
  assert_contains "$STDERR_LOG" "checksum verification failed"
  assert_existing_binary_untouched
}

test_archive_shape_failure_preserves_existing_binary() {
  new_case bad-archive
  make_release 0.8.0 0.8.0 native
  write_existing_binary
  printf 'unexpected\n' >"$CASE_DIR/payload/unexpected.txt"
  tar -czf "$ASSET_DIR/heyfood-v0.8.0-$(host_target).tar.gz" \
    -C "$CASE_DIR/payload" heyfood unexpected.txt
  refresh_native_checksums 0.8.0
  if run_installer; then
    fail "installer accepted unexpected archive members"
  fi
  assert_contains "$STDERR_LOG" "must contain only heyfood at its root"
  assert_existing_binary_untouched
}

test_version_mismatch_preserves_existing_binary() {
  new_case wrong-version
  make_release 0.8.0 9.9.9 native
  write_existing_binary
  if run_installer; then
    fail "installer accepted an executable with the wrong version"
  fi
  assert_contains "$STDERR_LOG" "expected heyfood 0.8.0 before installation"
  assert_existing_binary_untouched
}

test_current_installer_refuses_v062_before_download_and_preserves_state() {
  local binary_digest floor_digest state_digest state_sentinel
  new_case current-installer-v062-refusal
  use_native_state_installer
  write_existing_current_binary
  write_native_state_floor
  state_sentinel="$HEYFOOD_STATE_DIR/data/accounts/test-account/household-state.enc"
  mkdir -p "$(dirname "$state_sentinel")"
  printf 'opaque encrypted household state sentinel\n' >"$state_sentinel"
  binary_digest=$(sha256_file "$BIN_DIR/heyfood")
  floor_digest=$(sha256_file \
    "$HEYFOOD_STATE_DIR/data/compatibility/native-state-floor.v1.json")
  state_digest=$(sha256_file "$state_sentinel")
  export HEYFOOD_VERSION=0.6.2
  if run_installer; then
    fail "the current v0.8.0 installer accepted a v0.6.2 request"
  fi

  [[ ! -e "$DOWNLOAD_LOG" ]] ||
    fail "the current installer downloaded before rejecting the v0.6.2 request"
  assert_contains "$STDERR_LOG" \
    "this installer supports heyfood 0.8.0; requested 0.6.2"
  [[ "$("$BIN_DIR/heyfood" --version)" == "heyfood 0.8.0" ]] ||
    fail "the rejected v0.6.2 request replaced the installed v0.8.0 executable"
  [[ "$(sha256_file "$BIN_DIR/heyfood")" == "$binary_digest" ]] ||
    fail "the rejected v0.6.2 request changed the installed v0.8.0 bytes"
  [[ "$(sha256_file \
    "$HEYFOOD_STATE_DIR/data/compatibility/native-state-floor.v1.json")" == \
    "$floor_digest" ]] ||
    fail "the rejected v0.6.2 request changed the native-state floor"
  [[ "$(sha256_file "$state_sentinel")" == "$state_digest" ]] ||
    fail "the rejected v0.6.2 request changed account household state"
}

test_verified_native_state_install_and_compatible_reinstall() {
  new_case native-state-install
  use_native_state_installer
  make_release 0.8.0 0.8.0 native
  write_existing_binary
  run_installer
  [[ "$("$BIN_DIR/heyfood" --version)" == "heyfood 0.8.0" ]] ||
    fail "the standalone verifier prevented an exact no-floor install"
  assert_contains "$DOWNLOAD_LOG" "heyfood-v0.8.0-native-state.json"
  assert_contains "$DOWNLOAD_LOG" "heyfood-installer-v0.8.0-$(host_target).tar.gz"

  new_case compatible-floor
  use_native_state_installer
  make_release 0.8.0 0.8.0 native
  write_existing_binary
  write_native_state_floor
  run_installer
  [[ "$("$BIN_DIR/heyfood" --version)" == "heyfood 0.8.0" ]] ||
    fail "a compatible native-state-aware reinstall was rejected"
}

test_malformed_native_state_floor_rejects_before_download() {
  new_case malformed-floor
  use_native_state_installer
  make_release 0.8.0 0.8.0 native
  write_existing_binary
  write_native_state_floor
  printf '\n' >>"$HEYFOOD_STATE_DIR/data/compatibility/native-state-floor.v1.json"
  if run_installer; then
    fail "installer accepted a noncanonical native state floor"
  fi

  [[ ! -e "$DOWNLOAD_LOG" ]] ||
    fail "installer downloaded before validating the native state floor"
  assert_contains "$STDERR_LOG" "compatibility floor is malformed"
  assert_existing_binary_untouched
}

test_pre_native_state_and_disagreeing_metadata_preserve_existing_binary() {
  new_case pre-native-state-release
  use_native_state_installer
  make_release 0.8.0 0.8.0 native exact 1 1
  write_existing_binary
  if run_installer; then
    fail "installer accepted a pre-native-state release declaration"
  fi
  assert_contains "$STDERR_LOG" "release declaration is incompatible"
  assert_existing_binary_untouched

  new_case metadata-disagreement
  use_native_state_installer
  make_release 0.8.0 0.8.0 native exact 1 2
  write_existing_binary
  if run_installer; then
    fail "installer accepted release and binary metadata disagreement"
  fi
  assert_contains "$STDERR_LOG" "candidate and release native state declarations disagree"
  assert_existing_binary_untouched
}

test_structurally_invalid_candidate_manifests_preserve_existing_binary() {
  local variant
  for variant in nested textual duplicate-top duplicate-nested wrong-schema invalid oversized; do
    new_case "candidate-$variant"
    use_native_state_installer
    make_release 0.8.0 0.8.0 native "$variant"
    write_existing_binary
    if run_installer; then
      fail "installer accepted the $variant candidate manifest"
    fi
    assert_contains "$STDERR_LOG" "standalone verifier rejected"
    assert_existing_binary_untouched
  done
}

test_invalid_or_oversized_declarations_preserve_existing_binary() {
  new_case duplicate-declaration
  use_native_state_installer
  make_release 0.8.0 0.8.0 native
  write_existing_binary
  printf '%s' \
    '{"binary_version":"0.8.0","binary_version":"0.8.0","maximum_native_state_version":2,"native_state_capabilities":["household-account-slot-v1","household-lifecycle-lock-v1","household-migration-guard-v1","household-teardown-journal-v1"],"schema_version":1}' \
    >"$ASSET_DIR/heyfood-v0.8.0-native-state.json"
  refresh_native_checksums 0.8.0
  if run_installer; then
    fail "installer accepted a duplicate-key release declaration"
  fi
  assert_contains "$STDERR_LOG" "release declaration is malformed"
  assert_existing_binary_untouched

  new_case oversized-declaration
  use_native_state_installer
  make_release 0.8.0 0.8.0 native
  write_existing_binary
  {
    printf '%s' '{"padding":"'
    head -c 4096 /dev/zero | tr '\0' x
    printf '%s' '"}'
  } >"$ASSET_DIR/heyfood-v0.8.0-native-state.json"
  refresh_native_checksums 0.8.0
  if run_installer; then
    fail "installer accepted an oversized release declaration"
  fi
  assert_contains "$STDERR_LOG" "release declaration is unavailable"
  assert_existing_binary_untouched
}

test_missing_or_failing_verifier_preserves_existing_binary() {
  new_case missing-metadata
  use_native_state_installer
  make_release 0.8.0 0.8.0 native
  write_existing_binary
  rm "$ASSET_DIR/heyfood-v0.8.0-native-state.json"
  if run_installer; then
    fail "installer accepted a release without native state metadata"
  fi
  assert_contains "$STDERR_LOG" "could not download native state metadata"
  assert_existing_binary_untouched

  new_case missing-verifier
  use_native_state_installer
  make_release 0.8.0 0.8.0 native
  write_existing_binary
  rm "$ASSET_DIR/heyfood-installer-v0.8.0-$(host_target).tar.gz"
  if run_installer; then
    fail "installer accepted a release without the standalone verifier"
  fi
  assert_contains "$STDERR_LOG" "could not download the standalone native-state verifier"
  assert_existing_binary_untouched

  new_case rejecting-verifier
  use_native_state_installer
  make_release 0.8.0 0.8.0 native exact 2 2 reject
  write_existing_binary
  if run_installer; then
    fail "installer ignored a verifier rejection"
  fi
  assert_contains "$STDERR_LOG" "standalone verifier rejected"
  assert_existing_binary_untouched
}

test_interruption_before_replacement_preserves_existing_binary() {
  new_case verifier-interruption
  use_native_state_installer
  make_release 0.8.0 0.8.0 native exact 2 2 interrupt
  write_existing_binary
  if run_installer; then
    fail "interrupted installer reported success"
  fi
  assert_existing_binary_untouched
}

test_source_invariants
test_native_root_digest_vectors
test_exact_native_install
test_default_release_is_current
test_streamed_install
test_rejects_unsupported_or_unsafe_version_before_download
test_rejects_uncontrolled_install_targets
test_migrates_known_legacy_pipx_symlink
test_failed_install_preserves_known_legacy_pipx_symlink
test_checksum_failure_preserves_existing_binary
test_archive_shape_failure_preserves_existing_binary
test_version_mismatch_preserves_existing_binary
test_current_installer_refuses_v062_before_download_and_preserves_state
test_verified_native_state_install_and_compatible_reinstall
test_malformed_native_state_floor_rejects_before_download
test_pre_native_state_and_disagreeing_metadata_preserve_existing_binary
test_structurally_invalid_candidate_manifests_preserve_existing_binary
test_invalid_or_oversized_declarations_preserve_existing_binary
test_missing_or_failing_verifier_preserves_existing_binary
test_interruption_before_replacement_preserves_existing_binary

printf 'installer contract: all tests passed for %s\n' "$(host_target)"
