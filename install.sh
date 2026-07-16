#!/usr/bin/env bash

set -euo pipefail
IFS=$'\n\t'

readonly HEYFOOD_PACKAGE="heyfood-cli"
readonly HEYFOOD_COMMAND="heyfood"
readonly PYPI_INDEX_URL="https://pypi.org/simple"

say() {
  printf '%s\n' "$*"
}

fail() {
  printf 'heyfood installer: %s\n' "$*" >&2
  exit 1
}

validate_absolute_directory_variable() {
  local name="$1"
  local value="$2"

  if [[ -n "$value" && "$value" != /* ]]; then
    fail "$name must be an absolute path when set"
  fi
}

resolve_command() {
  local requested="$1"

  if [[ "$requested" == */* ]]; then
    [[ -x "$requested" ]] || return 1
    printf '%s\n' "$requested"
    return 0
  fi

  case "$requested" in
    "" | -* | *[!A-Za-z0-9._+-]*) return 1 ;;
  esac
  command -v "$requested" 2>/dev/null
}

python_is_supported() {
  "$1" -c 'import sys; raise SystemExit(0 if (3, 11) <= sys.version_info[:2] < (3, 14) else 1)' \
    </dev/null >/dev/null 2>&1
}

pipx_is_supported() {
  local version

  version=$("$@" --version </dev/null 2>/dev/null) || return 1
  "$PYTHON" -c '
import re
import sys

match = re.search(r"(?<![0-9])(\d+)\.(\d+)", sys.argv[1])
if match is None:
    raise SystemExit(1)
version = tuple(map(int, match.groups()))
raise SystemExit(0 if (1, 7) <= version < (2, 0) else 1)
' "$version" </dev/null >/dev/null 2>&1
}

if [[ "$#" -ne 0 ]]; then
  fail "this script takes no arguments; use the documented HEYFOOD_* environment variables"
fi

[[ "${HOME:-}" == /* && -d "$HOME" ]] ||
  fail "HOME must name an existing absolute directory"

if [[ "${EUID:-0}" == "0" ]]; then
  fail "do not run this installer with sudo or as root"
fi

case "$(uname -s 2>/dev/null || true)" in
  Darwin | Linux) ;;
  *) fail "only macOS and Linux are currently supported" ;;
esac

validate_absolute_directory_variable "PIPX_HOME" "${PIPX_HOME:-}"
validate_absolute_directory_variable "PIPX_BIN_DIR" "${PIPX_BIN_DIR:-}"
validate_absolute_directory_variable "XDG_DATA_HOME" "${XDG_DATA_HOME:-}"
validate_absolute_directory_variable "XDG_BIN_HOME" "${XDG_BIN_HOME:-}"

PYTHON=""
if [[ -n "${HEYFOOD_PYTHON:-}" ]]; then
  PYTHON=$(resolve_command "$HEYFOOD_PYTHON") ||
    fail "HEYFOOD_PYTHON does not name an executable Python interpreter"
else
  for candidate in python3.13 python3.12 python3.11 python3; do
    candidate_path=$(resolve_command "$candidate" || true)
    if [[ -n "$candidate_path" ]] && python_is_supported "$candidate_path"; then
      PYTHON="$candidate_path"
      break
    fi
  done
fi

[[ -n "$PYTHON" ]] ||
  fail "Python 3.11, 3.12, or 3.13 is required; install it, then run this command again"
python_is_supported "$PYTHON" ||
  fail "the selected interpreter must be Python 3.11, 3.12, or 3.13"

readonly PYTHON
PYTHON_VERSION=$("$PYTHON" -c 'import platform; print(platform.python_version())' </dev/null)
readonly PYTHON_VERSION

VERSION="${HEYFOOD_VERSION:-}"
if [[ -n "$VERSION" ]] &&
  ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+((a|b|rc)[0-9]+)?$ ]]; then
  fail "HEYFOOD_VERSION must be an exact release such as 0.2.0 or 1.0.0rc1"
fi

WITH_KEYRING="0"
case "${HEYFOOD_WITH_KEYRING:-0}" in
  0 | false | FALSE | False | no | NO | No) ;;
  1 | true | TRUE | True | yes | YES | Yes) WITH_KEYRING="1" ;;
  *) fail "HEYFOOD_WITH_KEYRING must be 0 or 1 when set" ;;
esac

REQUIREMENT="$HEYFOOD_PACKAGE"
if [[ "$WITH_KEYRING" == "1" ]]; then
  REQUIREMENT="${REQUIREMENT}[keyring]"
fi
if [[ -n "$VERSION" ]]; then
  REQUIREMENT="${REQUIREMENT}==${VERSION}"
fi
readonly REQUIREMENT

if [[ -n "${PIPX_BIN_DIR:-}" ]]; then
  HEYFOOD_BIN_DIR="$PIPX_BIN_DIR"
elif [[ -n "${XDG_BIN_HOME:-}" ]]; then
  HEYFOOD_BIN_DIR="$XDG_BIN_HOME"
else
  HEYFOOD_BIN_DIR="$HOME/.local/bin"
fi
readonly HEYFOOD_BIN_DIR
export PIPX_BIN_DIR="$HEYFOOD_BIN_DIR"

# Keep pip and pipx on the public PyPI index even when the invoking shell has a
# custom package index or requirements file configured. Proxy and CA variables
# remain available for legitimate network environments.
export PIP_CONFIG_FILE=/dev/null
export PIP_INDEX_URL="$PYPI_INDEX_URL"
export PIPX_DEFAULT_BACKEND=pip
export PIPX_FETCH_PYTHON=never
unset PIP_EXTRA_INDEX_URL PIP_FIND_LINKS PIP_TRUSTED_HOST PIP_NO_INDEX
unset PIP_CONSTRAINT PIP_BUILD_CONSTRAINT PIP_REQUIREMENT
unset PIP_TARGET PIP_PREFIX PIP_ROOT PIP_USER PIP_REQUIRE_HASHES
unset UV_INDEX UV_DEFAULT_INDEX UV_EXTRA_INDEX_URL UV_FIND_LINKS UV_INSECURE_HOST

PIPX_CMD=()
PIPX_EXECUTABLE=$(resolve_command pipx || true)
if [[ -n "$PIPX_EXECUTABLE" ]] && pipx_is_supported "$PIPX_EXECUTABLE"; then
  PIPX_CMD=("$PIPX_EXECUTABLE")
elif pipx_is_supported "$PYTHON" -m pipx; then
  PIPX_CMD=("$PYTHON" -m pipx)
fi

if [[ "${#PIPX_CMD[@]}" -eq 0 ]]; then
  if [[ -n "$PIPX_EXECUTABLE" ]]; then
    say "An unsupported pipx was found; using an isolated pipx bootstrap instead."
  fi

  if [[ -n "${XDG_DATA_HOME:-}" ]]; then
    BOOTSTRAP_DATA_HOME="$XDG_DATA_HOME"
  else
    BOOTSTRAP_DATA_HOME="$HOME/.local/share"
  fi
  readonly BOOTSTRAP_DATA_HOME
  readonly BOOTSTRAP_DIR="$BOOTSTRAP_DATA_HOME/heyfood/installer/pipx"
  readonly BOOTSTRAP_PYTHON="$BOOTSTRAP_DIR/bin/python"

  if [[ ! -e "$BOOTSTRAP_DIR" ]]; then
    say "Preparing an isolated pipx bootstrap in $BOOTSTRAP_DIR"
    mkdir -p "$(dirname "$BOOTSTRAP_DIR")"
    (umask 077 && "$PYTHON" -m venv "$BOOTSTRAP_DIR" </dev/null) ||
      fail "Python could not create a virtual environment; install its venv support and try again"
  elif [[ ! -x "$BOOTSTRAP_PYTHON" ]]; then
    fail "the existing pipx bootstrap is incomplete: $BOOTSTRAP_DIR"
  fi

  if ! pipx_is_supported "$BOOTSTRAP_PYTHON" -m pipx; then
    say "Installing the isolated pipx bootstrap from PyPI."
    "$BOOTSTRAP_PYTHON" -m pip --isolated install \
      --disable-pip-version-check \
      --no-input \
      --index-url "$PYPI_INDEX_URL" \
      'pipx>=1.7,<2' </dev/null
  fi

  pipx_is_supported "$BOOTSTRAP_PYTHON" -m pipx ||
    fail "the isolated pipx bootstrap did not verify"
  PIPX_CMD=("$BOOTSTRAP_PYTHON" -m pipx)
fi

mkdir -p "$HEYFOOD_BIN_DIR"

say "Installing $REQUIREMENT from PyPI with Python $PYTHON_VERSION."
"${PIPX_CMD[@]}" install \
  --quiet \
  --force \
  --python "$PYTHON" \
  --index-url "$PYPI_INDEX_URL" \
  --pip-args="--disable-pip-version-check --no-input" \
  "$REQUIREMENT" </dev/null

readonly HEYFOOD_EXECUTABLE="$HEYFOOD_BIN_DIR/$HEYFOOD_COMMAND"
[[ -x "$HEYFOOD_EXECUTABLE" ]] ||
  fail "pipx completed without creating $HEYFOOD_EXECUTABLE"

VERSION_OUTPUT=$("$HEYFOOD_EXECUTABLE" --version </dev/null 2>&1) ||
  fail "the installed heyfood command did not start successfully"
if [[ -n "$VERSION" ]]; then
  [[ "$VERSION_OUTPUT" == "heyfood $VERSION" ]] ||
    fail "expected heyfood $VERSION after installation, received: $VERSION_OUTPUT"
elif ! [[ "$VERSION_OUTPUT" =~ ^heyfood\ [0-9]+\.[0-9]+\.[0-9]+((a|b|rc)[0-9]+)?$ ]]; then
  fail "the installed command returned an unexpected version: $VERSION_OUTPUT"
fi

say ""
say "Installed $VERSION_OUTPUT"
case ":${PATH:-}:" in
  *":$HEYFOOD_BIN_DIR:"*) ;;
  *)
    say "Add heyfood to this shell's PATH:"
    printf '  export PATH=%q:$PATH\n' "$HEYFOOD_BIN_DIR"
    ;;
esac
say "Next: heyfood login"
printf 'Uninstall:'
printf ' %q' "${PIPX_CMD[@]}"
printf ' uninstall %q\n' "$HEYFOOD_PACKAGE"
