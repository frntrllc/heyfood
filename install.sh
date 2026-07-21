#!/usr/bin/env bash

set -euo pipefail
IFS=$'\n\t'

readonly HEYFOOD_COMMAND="heyfood"
readonly HEYFOOD_REPOSITORY="frntrllc/heyfood"
readonly GITHUB_URL="https://github.com"
readonly RELEASE_URL="$GITHUB_URL/$HEYFOOD_REPOSITORY/releases"

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

VERSION="${HEYFOOD_VERSION:-}"
if [[ -n "$VERSION" ]]; then
  validate_version "$VERSION" ||
    fail "HEYFOOD_VERSION must be an exact release such as 0.4.0 or 0.4.0-rc.1"
else
  LATEST_URL=$(
    "$CURL" -qfsSL \
      --proto '=https' \
      --tlsv1.2 \
      --retry 3 \
      --output /dev/null \
      --write-out '%{url_effective}' \
      "$RELEASE_URL/latest"
  ) || fail "could not resolve the latest heyfood release"
  case "$LATEST_URL" in
    "$RELEASE_URL/tag/v"*) VERSION="${LATEST_URL##*/}"; VERSION="${VERSION#v}" ;;
    *) fail "the latest-release redirect did not resolve to a heyfood version tag" ;;
  esac
  validate_version "$VERSION" || fail "the latest heyfood release tag is not a valid version"
fi
readonly VERSION

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
[[ ! -L "$INSTALL_PATH" ]] ||
  fail "refusing to replace a symbolic link: $INSTALL_PATH"
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
readonly DOWNLOAD_BASE="$RELEASE_URL/download/v$VERSION"
readonly ARCHIVE_PATH="$TEMP_DIR/$ARCHIVE"
readonly CHECKSUMS_PATH="$TEMP_DIR/$CHECKSUMS"

say "Downloading heyfood $VERSION for $TARGET."
download "$DOWNLOAD_BASE/$CHECKSUMS" "$CHECKSUMS_PATH" ||
  fail "could not download release checksums for heyfood $VERSION"
download "$DOWNLOAD_BASE/$ARCHIVE" "$ARCHIVE_PATH" ||
  fail "could not download the native heyfood release for $TARGET"

EXPECTED_CHECKSUM=$(
  awk -v archive="$ARCHIVE" 'NF == 2 && $2 == archive { print $1 }' "$CHECKSUMS_PATH"
)
[[ "$EXPECTED_CHECKSUM" =~ ^[0-9A-Fa-f]{64}$ ]] ||
  fail "release checksums do not contain exactly one valid entry for $ARCHIVE"

if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_CHECKSUM=$(sha256sum "$ARCHIVE_PATH" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL_CHECKSUM=$(shasum -a 256 "$ARCHIVE_PATH" | awk '{ print $1 }')
else
  fail "sha256sum or shasum is required to verify the native release"
fi
EXPECTED_CHECKSUM=$(printf '%s' "$EXPECTED_CHECKSUM" | tr 'A-F' 'a-f')
ACTUAL_CHECKSUM=$(printf '%s' "$ACTUAL_CHECKSUM" | tr 'A-F' 'a-f')
[[ "$ACTUAL_CHECKSUM" == "$EXPECTED_CHECKSUM" ]] ||
  fail "checksum verification failed for $ARCHIVE"

ARCHIVE_MEMBERS=$(tar -tzf "$ARCHIVE_PATH") || fail "the release archive is invalid"
[[ "$ARCHIVE_MEMBERS" == "$HEYFOOD_COMMAND" ]] ||
  fail "the release archive must contain only the heyfood executable at its root"
ARCHIVE_DETAILS=$(tar -tvzf "$ARCHIVE_PATH") || fail "the release archive is invalid"
[[ "${ARCHIVE_DETAILS:0:1}" == "-" ]] ||
  fail "the heyfood archive member must be a regular file"

(umask 077 && tar -xzf "$ARCHIVE_PATH" -C "$TEMP_DIR" -- "$HEYFOOD_COMMAND") ||
  fail "could not unpack the native heyfood executable"
readonly STAGED_EXECUTABLE="$TEMP_DIR/$HEYFOOD_COMMAND"
[[ -f "$STAGED_EXECUTABLE" && ! -L "$STAGED_EXECUTABLE" ]] ||
  fail "the unpacked heyfood executable is not a regular file"
chmod 0755 "$STAGED_EXECUTABLE"

VERSION_OUTPUT=$("$STAGED_EXECUTABLE" --version 2>&1) ||
  fail "the downloaded heyfood executable did not start successfully"
[[ "$VERSION_OUTPUT" == "heyfood $VERSION" ]] ||
  fail "expected heyfood $VERSION before installation, received: $VERSION_OUTPUT"

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
say "Connect an account: heyfood register"
printf 'Uninstall: rm %q\n' "$INSTALL_PATH"
