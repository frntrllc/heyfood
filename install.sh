#!/usr/bin/env bash

set -euo pipefail
IFS=$'\n\t'

readonly HEYFOOD_COMMAND="heyfood"
readonly HEYFOOD_REPOSITORY="frntrllc/heyfood"
readonly GITHUB_URL="https://github.com"
readonly RELEASE_URL="$GITHUB_URL/$HEYFOOD_REPOSITORY/releases"
readonly SUPPORTED_VERSION="0.7.0"
readonly NATIVE_STATE_RELEASE_VERSION="0.7.0"
readonly NATIVE_STATE_VERSION="2"
readonly NATIVE_STATE_CAPABILITIES='["household-account-slot-v1","household-lifecycle-lock-v1","household-migration-guard-v1","household-teardown-journal-v1"]'

say() {
  printf '%s\n' "$*"
}

fail() {
  printf 'heyfood installer: %s\n' "$*" >&2
  exit 1
}

validate_version() {
  local version="$1"

  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?$ ]]
}

directory_owner_uid() {
  case "$1" in
    Darwin) stat -f '%u' "$2" ;;
    Linux) stat -c '%u' "$2" ;;
    *) return 1 ;;
  esac
}

directory_mode() {
  case "$1" in
    Darwin) stat -f '%Lp' "$2" ;;
    Linux) stat -c '%a' "$2" ;;
    *) return 1 ;;
  esac
}

sha256_file() {
  case "$SHA256_TOOL" in
    sha256sum) sha256sum "$1" | awk '{ print $1 }' ;;
    shasum) shasum -a 256 "$1" | awk '{ print $1 }' ;;
    *) return 1 ;;
  esac
}

sha256_stdin() {
  case "$SHA256_TOOL" in
    sha256sum) sha256sum | awk '{ print $1 }' ;;
    shasum) shasum -a 256 | awk '{ print $1 }' ;;
    *) return 1 ;;
  esac
}

byte_length() {
  LC_ALL=C printf '%s' "$1" | wc -c | tr -d '[:space:]'
}

write_u32_be() {
  local value="$1"
  local encoded

  [[ "$value" =~ ^[0-9]+$ ]] || return 1
  ((value <= 4294967295)) || return 1
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

verify_private_path() {
  local expected_mode="$1"
  local path="$2"
  local description="$3"
  local owner mode

  [[ ! -L "$path" ]] || fail "$description must not be a symbolic link: $path"
  owner=$(directory_owner_uid "$OS" "$path") ||
    fail "could not verify ownership of $description"
  [[ "$owner" == "$CURRENT_UID" ]] ||
    fail "$description must be owned by the current user: $path"
  mode=$(directory_mode "$OS" "$path") ||
    fail "could not verify permissions of $description"
  [[ "$mode" == "$expected_mode" ]] ||
    fail "$description must have mode $expected_mode: $path"
}

inspect_native_state_floor() {
  local native_root floor_directory floor_path physical_root root_digest expected_floor

  NATIVE_STATE_FLOOR_PATH="-"
  NATIVE_ROOT_DIGEST="-"
  NATIVE_STATE_FLOOR_PRESENT="false"

  if [[ -n "${HEYFOOD_STATE_DIR:-}" ]]; then
    [[ "$HEYFOOD_STATE_DIR" == /* ]] ||
      fail "HEYFOOD_STATE_DIR must be an absolute path when set"
    native_root="$HEYFOOD_STATE_DIR/data"
  elif [[ "$OS" == "Darwin" ]]; then
    native_root="$HOME/Library/Application Support/ai.frntr.heyfood"
  else
    if [[ -n "${XDG_DATA_HOME:-}" && "$XDG_DATA_HOME" == /* ]]; then
      native_root="$XDG_DATA_HOME/heyfood"
    else
      native_root="$HOME/.local/share/heyfood"
    fi
  fi

  case "$native_root" in
    *$'\n'* | *$'\r'*) fail "the native state root contains unsupported control characters" ;;
  esac
  floor_directory="$native_root/compatibility"
  floor_path="$floor_directory/native-state-floor.v1.json"
  [[ -e "$floor_path" || -L "$floor_path" ]] || return 0

  [[ -d "$native_root" ]] || fail "the native state root is not a directory: $native_root"
  verify_private_path "700" "$native_root" "the native state root"
  [[ -d "$floor_directory" ]] ||
    fail "the native state compatibility directory is not a directory"
  verify_private_path "700" "$floor_directory" "the native state compatibility directory"
  [[ -f "$floor_path" ]] || fail "the native state compatibility floor is not a regular file"
  verify_private_path "600" "$floor_path" "the native state compatibility floor"
  [[ "$(wc -c <"$floor_path" | tr -d '[:space:]')" -le 4096 ]] ||
    fail "the native state compatibility floor exceeds 4096 bytes"

  physical_root=$(cd "$native_root" && pwd -P) ||
    fail "could not resolve the native state root"
  root_digest=$(native_root_instance_digest \
    "$([[ "$OS" == "Darwin" ]] && printf 'macos' || printf 'linux')" \
    "$physical_root") ||
    fail "could not calculate the native state root identity"
  expected_floor="$TEMP_DIR/native-state-floor.expected.json"
  printf '%s' \
    "{\"floor_revision\":1,\"minimum_compatible_native_state_version\":$NATIVE_STATE_VERSION,\"native_root_instance_digest\":\"$root_digest\",\"required_binary_capabilities\":$NATIVE_STATE_CAPABILITIES,\"schema_version\":1}" \
    >"$expected_floor"
  cmp -s "$expected_floor" "$floor_path" ||
    fail "the native state compatibility floor is malformed or belongs to another root"

  NATIVE_STATE_FLOOR_PATH="$floor_path"
  NATIVE_ROOT_DIGEST="$root_digest"
  NATIVE_STATE_FLOOR_PRESENT="true"
}

verify_downloaded_checksum() {
  local asset="$1"
  local path="$2"
  local expected actual

  expected=$(
    awk -v asset="$asset" 'NF == 2 && $2 == asset { print $1 }' "$CHECKSUMS_PATH"
  )
  [[ "$expected" =~ ^[0-9A-Fa-f]{64}$ ]] ||
    fail "release checksums do not contain exactly one valid entry for $asset"
  actual=$(sha256_file "$path")
  expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')
  actual=$(printf '%s' "$actual" | tr 'A-F' 'a-f')
  [[ "$actual" == "$expected" ]] ||
    fail "checksum verification failed for $asset"
}

verify_archive_member() {
  local archive_path="$1"
  local member="$2"
  local members details

  members=$(tar -tzf "$archive_path") || fail "the release archive is invalid: $archive_path"
  [[ "$members" == "$member" ]] ||
    fail "the release archive must contain only $member at its root"
  details=$(tar -tvzf "$archive_path") ||
    fail "the release archive is invalid: $archive_path"
  [[ "${details:0:1}" == "-" ]] ||
    fail "the $member archive member must be a regular file"
}

download() {
  local url="$1"
  local destination="$2"

  "$CURL" -qfsSL \
    --proto '=https' \
    --tlsv1.2 \
    --retry 3 \
    --output "$destination" \
    "$url"
}

if [[ "$#" -ne 0 ]]; then
  fail "this script takes no arguments; use the documented HEYFOOD_* environment variables"
fi

[[ "${HOME:-}" == /* && -d "$HOME" ]] ||
  fail "HOME must name an existing absolute directory"

if [[ "${EUID:-0}" == "0" ]]; then
  fail "do not run this installer with sudo or as root"
fi

CURL=$(command -v curl 2>/dev/null || true)
[[ -n "$CURL" ]] || fail "curl is required to download the native release"
readonly CURL

command -v tar >/dev/null 2>&1 || fail "tar is required to unpack the native release"
if command -v sha256sum >/dev/null 2>&1; then
  SHA256_TOOL="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
  SHA256_TOOL="shasum"
else
  fail "sha256sum or shasum is required to verify the native release"
fi
readonly SHA256_TOOL

OS=$(uname -s 2>/dev/null || true)
case "$OS" in
  Darwin) TARGET_OS="apple-darwin" ;;
  Linux) TARGET_OS="unknown-linux-gnu" ;;
  *) fail "only macOS and Linux are currently supported" ;;
esac

ARCH=$(uname -m 2>/dev/null || true)
case "$ARCH" in
  arm64 | aarch64) TARGET_ARCH="aarch64" ;;
  x86_64 | amd64) TARGET_ARCH="x86_64" ;;
  *) fail "unsupported CPU architecture: $ARCH" ;;
esac

readonly OS TARGET_OS ARCH TARGET_ARCH
readonly TARGET="$TARGET_ARCH-$TARGET_OS"

VERSION="${HEYFOOD_VERSION:-$SUPPORTED_VERSION}"
validate_version "$VERSION" ||
  fail "HEYFOOD_VERSION must be an exact release such as 0.7.0"
[[ "$VERSION" == "$SUPPORTED_VERSION" ]] ||
  fail "this installer supports heyfood $SUPPORTED_VERSION; requested $VERSION"
readonly VERSION
if [[ -n "$NATIVE_STATE_RELEASE_VERSION" &&
  "$NATIVE_STATE_RELEASE_VERSION" != "$SUPPORTED_VERSION" ]]; then
  fail "the installer native-state release boundary is internally inconsistent"
fi
NATIVE_STATE_VERIFICATION_ACTIVE="false"
if [[ -n "$NATIVE_STATE_RELEASE_VERSION" &&
  "$VERSION" == "$NATIVE_STATE_RELEASE_VERSION" ]]; then
  NATIVE_STATE_VERIFICATION_ACTIVE="true"
fi
readonly NATIVE_STATE_VERIFICATION_ACTIVE

if [[ -n "${HEYFOOD_BIN_DIR:-}" ]]; then
  BIN_DIR="$HEYFOOD_BIN_DIR"
  BIN_DIR_VARIABLE="HEYFOOD_BIN_DIR"
elif [[ -n "${XDG_BIN_HOME:-}" ]]; then
  BIN_DIR="$XDG_BIN_HOME"
  BIN_DIR_VARIABLE="XDG_BIN_HOME"
else
  BIN_DIR="$HOME/.local/bin"
  BIN_DIR_VARIABLE="default installation directory"
fi
[[ "$BIN_DIR" == /* ]] || fail "$BIN_DIR_VARIABLE must be an absolute path when set"
[[ ! -L "$BIN_DIR" ]] || fail "the installation directory must not be a symbolic link: $BIN_DIR"

(umask 077 && mkdir -p "$BIN_DIR")
[[ -d "$BIN_DIR" && ! -L "$BIN_DIR" ]] ||
  fail "could not create a private installation directory: $BIN_DIR"

CURRENT_UID=$(id -u)
BIN_OWNER_UID=$(directory_owner_uid "$OS" "$BIN_DIR") ||
  fail "could not verify ownership of the installation directory"
[[ "$BIN_OWNER_UID" == "$CURRENT_UID" ]] ||
  fail "the installation directory must be owned by the current user: $BIN_DIR"
BIN_MODE=$(directory_mode "$OS" "$BIN_DIR") ||
  fail "could not verify permissions of the installation directory"
[[ "$BIN_MODE" =~ ^[0-7]{3,4}$ ]] ||
  fail "the installation directory returned an invalid permission mode"
BIN_WRITE_BITS="${BIN_MODE: -2}"
if [[ "${BIN_WRITE_BITS:0:1}" =~ [2367] || "${BIN_WRITE_BITS:1:1}" =~ [2367] ]]; then
  fail "the installation directory must not be writable by group or other users: $BIN_DIR"
fi
readonly BIN_DIR BIN_DIR_VARIABLE CURRENT_UID BIN_OWNER_UID BIN_MODE BIN_WRITE_BITS

readonly INSTALL_PATH="$BIN_DIR/$HEYFOOD_COMMAND"
readonly LEGACY_PIPX_TARGET="$HOME/.local/pipx/venvs/heyfood-cli/bin/heyfood"
LEGACY_PIPX_LINK=""
if [[ -L "$INSTALL_PATH" ]]; then
  LEGACY_PIPX_LINK=$(readlink "$INSTALL_PATH") ||
    fail "could not inspect the existing heyfood symbolic link"
  [[ "$LEGACY_PIPX_LINK" == "$LEGACY_PIPX_TARGET" ]] ||
    fail "refusing to replace an unrecognized symbolic link: $INSTALL_PATH"
fi
readonly LEGACY_PIPX_LINK
[[ ! -e "$INSTALL_PATH" || -f "$INSTALL_PATH" ]] ||
  fail "refusing to replace a non-file: $INSTALL_PATH"

TEMP_DIR=$(mktemp -d "$BIN_DIR/.heyfood-install.XXXXXX") ||
  fail "could not create a private staging directory"
readonly TEMP_DIR
cleanup() {
  rm -rf -- "$TEMP_DIR"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

readonly ARCHIVE="heyfood-v$VERSION-$TARGET.tar.gz"
readonly CHECKSUMS="SHA256SUMS"
readonly NATIVE_STATE_DECLARATION="heyfood-v$VERSION-native-state.json"
readonly VERIFIER_ARCHIVE="heyfood-installer-v$VERSION-$TARGET.tar.gz"
readonly DOWNLOAD_BASE="$RELEASE_URL/download/v$VERSION"
readonly ARCHIVE_PATH="$TEMP_DIR/$ARCHIVE"
readonly CHECKSUMS_PATH="$TEMP_DIR/$CHECKSUMS"
readonly NATIVE_STATE_DECLARATION_PATH="$TEMP_DIR/$NATIVE_STATE_DECLARATION"
readonly VERIFIER_ARCHIVE_PATH="$TEMP_DIR/$VERIFIER_ARCHIVE"

inspect_native_state_floor
readonly NATIVE_STATE_FLOOR_PATH NATIVE_ROOT_DIGEST NATIVE_STATE_FLOOR_PRESENT
if [[ "$NATIVE_STATE_FLOOR_PRESENT" == "true" &&
  "$NATIVE_STATE_VERIFICATION_ACTIVE" != "true" ]]; then
  fail "heyfood $VERSION predates the native-state compatibility floor; install a native-state-compatible release"
fi

say "Downloading heyfood $VERSION for $TARGET."
download "$DOWNLOAD_BASE/$CHECKSUMS" "$CHECKSUMS_PATH" ||
  fail "could not download release checksums for heyfood $VERSION"
download "$DOWNLOAD_BASE/$ARCHIVE" "$ARCHIVE_PATH" ||
  fail "could not download the native heyfood release for $TARGET"

[[ -f "$CHECKSUMS_PATH" && ! -L "$CHECKSUMS_PATH" ]] ||
  fail "release checksums are not a regular file"
CHECKSUMS_BYTES=$(wc -c <"$CHECKSUMS_PATH" | tr -d '[:space:]')
[[ "$CHECKSUMS_BYTES" =~ ^[0-9]+$ && "$CHECKSUMS_BYTES" -gt 0 &&
  "$CHECKSUMS_BYTES" -le 1048576 ]] ||
  fail "release checksums are empty or exceed 1048576 bytes"
verify_downloaded_checksum "$ARCHIVE" "$ARCHIVE_PATH"
verify_archive_member "$ARCHIVE_PATH" "$HEYFOOD_COMMAND"

CANDIDATE_DIRECTORY="$TEMP_DIR/candidate"
(umask 077 && mkdir "$CANDIDATE_DIRECTORY")
(umask 077 && tar -xzf "$ARCHIVE_PATH" -C "$CANDIDATE_DIRECTORY" -- "$HEYFOOD_COMMAND") ||
  fail "could not unpack the native heyfood executable"
readonly CANDIDATE_DIRECTORY
readonly STAGED_EXECUTABLE="$CANDIDATE_DIRECTORY/$HEYFOOD_COMMAND"
[[ -f "$STAGED_EXECUTABLE" && ! -L "$STAGED_EXECUTABLE" ]] ||
  fail "the unpacked heyfood executable is not a regular file"
chmod 0755 "$STAGED_EXECUTABLE"

VERSION_OUTPUT=$("$STAGED_EXECUTABLE" --version 2>&1) ||
  fail "the downloaded heyfood executable did not start successfully"
[[ "$VERSION_OUTPUT" == "heyfood $VERSION" ]] ||
  fail "expected heyfood $VERSION before installation, received: $VERSION_OUTPUT"

if [[ "$NATIVE_STATE_VERIFICATION_ACTIVE" == "true" ]]; then
  download "$DOWNLOAD_BASE/$NATIVE_STATE_DECLARATION" "$NATIVE_STATE_DECLARATION_PATH" ||
    fail "could not download native state metadata for heyfood $VERSION"
  download "$DOWNLOAD_BASE/$VERIFIER_ARCHIVE" "$VERIFIER_ARCHIVE_PATH" ||
    fail "could not download the standalone native-state verifier for $TARGET"

  verify_downloaded_checksum \
    "$NATIVE_STATE_DECLARATION" \
    "$NATIVE_STATE_DECLARATION_PATH"
  verify_downloaded_checksum "$VERIFIER_ARCHIVE" "$VERIFIER_ARCHIVE_PATH"
  verify_archive_member "$VERIFIER_ARCHIVE_PATH" "heyfood-installer"

  VERIFIER_DIRECTORY="$TEMP_DIR/verifier"
  (umask 077 && mkdir "$VERIFIER_DIRECTORY")
  (umask 077 && tar -xzf \
    "$VERIFIER_ARCHIVE_PATH" \
    -C "$VERIFIER_DIRECTORY" \
    -- "heyfood-installer") ||
    fail "could not unpack the standalone native-state verifier"
  readonly VERIFIER_DIRECTORY
  readonly STAGED_VERIFIER="$VERIFIER_DIRECTORY/heyfood-installer"
  [[ -f "$STAGED_VERIFIER" && ! -L "$STAGED_VERIFIER" ]] ||
    fail "the unpacked native-state verifier is not a regular file"
  chmod 0755 "$STAGED_VERIFIER"
  VERIFIER_VERSION_OUTPUT=$("$STAGED_VERIFIER" --version 2>&1) ||
    fail "the standalone native-state verifier did not start successfully"
  [[ "$VERIFIER_VERSION_OUTPUT" == "heyfood-installer $VERSION" ]] ||
    fail "expected heyfood-installer $VERSION, received: $VERIFIER_VERSION_OUTPUT"

  CANDIDATE_MANIFEST_PATH="$TEMP_DIR/candidate-agent-manifest.json"
  "$STAGED_EXECUTABLE" agent describe --schema-version 2 \
    >"$CANDIDATE_MANIFEST_PATH" 2>/dev/null ||
    fail "the downloaded heyfood executable did not expose native state metadata"
  readonly CANDIDATE_MANIFEST_PATH
  "$STAGED_VERIFIER" verify-native-state \
    "$VERSION" \
    "$NATIVE_ROOT_DIGEST" \
    "$NATIVE_STATE_FLOOR_PATH" \
    "$NATIVE_STATE_DECLARATION_PATH" \
    "$CANDIDATE_MANIFEST_PATH" ||
    fail "the standalone verifier rejected the downloaded heyfood executable"
fi

if [[ -n "$LEGACY_PIPX_LINK" ]]; then
  [[ -L "$INSTALL_PATH" ]] ||
    fail "the legacy heyfood symbolic link changed during installation"
  [[ "$(readlink "$INSTALL_PATH")" == "$LEGACY_PIPX_LINK" ]] ||
    fail "the legacy heyfood symbolic link target changed during installation"
fi

# The staging directory lives inside BIN_DIR, so this rename cannot cross a
# filesystem boundary. A reader sees either the prior complete executable or
# the newly verified complete executable, never a partially written binary.
mv -f -- "$STAGED_EXECUTABLE" "$INSTALL_PATH" ||
  fail "could not atomically install heyfood into $BIN_DIR"

say ""
say "Installed heyfood $VERSION at $INSTALL_PATH"
case ":${PATH:-}:" in
  *":$BIN_DIR:"*) ;;
  *)
    say "Add heyfood to this shell's PATH:"
    printf "  export PATH=%q:\$PATH\n" "$BIN_DIR"
    ;;
esac
say "Next: heyfood"
say "Open heyfood to sign in or create an account: heyfood"
say "Direct account commands: heyfood login (existing) or heyfood register (new)"
say "Agent contract: heyfood agent describe"
say "Optional Codex/Claude setup preview: heyfood agent setup --target all --scope user --dry-run"
printf 'Uninstall: rm %q\n' "$INSTALL_PATH"
