#!/usr/bin/env bash

set -euo pipefail
IFS=$'\n\t'

fail() {
  printf 'candidate transport: %s\n' "$*" >&2
  exit 1
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

serve_verified_asset() {
  [[ "$#" -eq 9 ]] || fail "the installer used an unexpected curl argument count"
  [[ "$1" == "-qfsSL" && "$2" == "--proto" && "$3" == "=https" &&
    "$4" == "--tlsv1.2" && "$5" == "--retry" && "$6" == "3" &&
    "$7" == "--output" ]] ||
    fail "the installer used an unexpected curl option"

  local destination=$8
  local url=$9
  local release_directory=${HEYFOOD_CANDIDATE_TRANSPORT_DIRECTORY:-}
  local version=${HEYFOOD_CANDIDATE_TRANSPORT_VERSION:-}
  local approved_manifest_sha256=${HEYFOOD_CANDIDATE_TRANSPORT_MANIFEST_SHA256:-}
  local expected_prefix="https://github.com/frntrllc/heyfood/releases/download/v$version/"
  local asset manifest source expected actual

  [[ "$release_directory" == /* && -d "$release_directory" ]] ||
    fail "the verified candidate directory is unavailable"
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
    fail "the candidate version is invalid"
  [[ "$approved_manifest_sha256" =~ ^[0-9a-f]{64}$ ]] ||
    fail "the approved SHA256SUMS SHA-256 is invalid"
  [[ "$destination" == /* && "$destination" != *$'\n'* && "$destination" != *$'\r'* ]] ||
    fail "the installer requested an unsafe destination"
  [[ ! -e "$destination" && ! -L "$destination" ]] ||
    fail "the installer destination already exists"
  [[ "$url" == "$expected_prefix"* ]] ||
    fail "the installer requested an unapproved release URL"

  asset=${url#"$expected_prefix"}
  [[ -n "$asset" && "$asset" != */* && "$asset" != *$'\n'* && "$asset" != *$'\r'* ]] ||
    fail "the installer requested an unsafe asset name"
  manifest="$release_directory/SHA256SUMS"
  [[ -f "$manifest" && ! -L "$manifest" ]] ||
    fail "the approved SHA256SUMS is unavailable"
  [[ "$(sha256_file "$manifest")" == "$approved_manifest_sha256" ]] ||
    fail "the approved SHA256SUMS digest changed"

  if [[ "$asset" == "SHA256SUMS" ]]; then
    source=$manifest
  else
    expected=$(awk -v asset="$asset" 'NF == 2 && $2 == asset { print $1 }' "$manifest")
    [[ "$expected" =~ ^[0-9a-f]{64}$ ]] ||
      fail "the requested asset is not uniquely bound by SHA256SUMS"
    source="$release_directory/$asset"
    [[ -f "$source" && ! -L "$source" ]] ||
      fail "the requested candidate asset is unavailable"
    actual=$(sha256_file "$source")
    [[ "$actual" == "$expected" ]] ||
      fail "the requested candidate asset digest changed"
  fi

  install -m 0600 "$source" "$destination"
}

refuse_asset_transport() {
  local invocation_marker=${HEYFOOD_CANDIDATE_TRANSPORT_INVOCATION_MARKER:-}

  [[ "$invocation_marker" == /* &&
    "$invocation_marker" != *$'\n'* &&
    "$invocation_marker" != *$'\r'* &&
    ! -e "$invocation_marker" &&
    ! -L "$invocation_marker" ]] ||
    fail "the expect-no-download invocation marker is unsafe"
  (umask 077 && printf 'curl invoked\n' >"$invocation_marker") ||
    fail "could not record the prohibited curl invocation"
  fail "the installer invoked curl in expect-no-download mode"
}

if [[ "${0##*/}" == "curl" ]]; then
  if [[ "${HEYFOOD_CANDIDATE_TRANSPORT_EXPECT_NO_DOWNLOAD:-}" == "1" ]]; then
    refuse_asset_transport
  fi
  serve_verified_asset "$@"
  exit 0
fi

expect_no_download=false
installer_request_version=
if [[ "${1:-}" == "--expect-no-download" ]]; then
  [[ "$#" -ge 2 ]] || fail "--expect-no-download requires an installer request version"
  expect_no_download=true
  installer_request_version=$2
  shift 2
fi

if [[ "$#" -ne 4 ]]; then
  echo "usage: candidate-transport.sh [--expect-no-download INSTALLER_REQUEST_VERSION] RELEASE_DIRECTORY APPROVED_VERSION SHA256SUMS_SHA256 INSTALLER" >&2
  exit 64
fi

release_directory_input=$1
version=$2
approved_manifest_sha256=$3
installer_input=$4

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  fail "APPROVED_VERSION must be an exact release version"
if [[ -z "$installer_request_version" ]]; then
  installer_request_version=$version
fi
[[ "$installer_request_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  fail "INSTALLER_REQUEST_VERSION must be an exact release version"
if [[ "$expect_no_download" == true && "$installer_request_version" == "$version" ]]; then
  fail "expect-no-download requires a request different from the approved version"
fi
[[ "$approved_manifest_sha256" =~ ^[0-9a-f]{64}$ ]] ||
  fail "SHA256SUMS_SHA256 must be a lowercase SHA-256 digest"
[[ -d "$release_directory_input" && ! -L "$release_directory_input" ]] ||
  fail "RELEASE_DIRECTORY must be a real directory"
release_directory=$(cd "$release_directory_input" && pwd -P)
[[ "$release_directory" != *$'\n'* && "$release_directory" != *$'\r'* ]] ||
  fail "RELEASE_DIRECTORY contains unsupported control characters"
[[ -f "$installer_input" && ! -L "$installer_input" ]] ||
  fail "INSTALLER must be a regular reviewed file"
installer_directory=$(cd "$(dirname "$installer_input")" && pwd -P)
installer="$installer_directory/$(basename "$installer_input")"

script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
script="$script_directory/$(basename "${BASH_SOURCE[0]}")"
verify_assets="$script_directory/verify-assets.sh"
[[ -x "$verify_assets" ]] || fail "the release-set verifier is unavailable"

"$verify_assets" \
  "$release_directory" \
  "$version" \
  --native-state-manifest-bound \
  "$approved_manifest_sha256" ||
  fail "the approved candidate release set did not verify"
[[ "$(sha256_file "$release_directory/SHA256SUMS")" == "$approved_manifest_sha256" ]] ||
  fail "the candidate release-set digest does not match the approved digest"
while IFS= read -r -d '' candidate_asset; do
  [[ -f "$candidate_asset" && ! -L "$candidate_asset" ]] ||
    fail "the candidate release set contains a non-regular asset"
done < <(find "$release_directory" -mindepth 1 -maxdepth 1 -print0)

transport_bin=$(mktemp -d "${TMPDIR:-/tmp}/heyfood-candidate-transport.XXXXXX")
cleanup() {
  rm -rf -- "$transport_bin"
}
trap cleanup EXIT HUP INT TERM
ln -s "$script" "$transport_bin/curl"

if [[ "$expect_no_download" == true ]]; then
  invocation_marker="$transport_bin/curl-invoked"
  refusal_stdout="$transport_bin/installer.stdout"
  refusal_stderr="$transport_bin/installer.stderr"
  expected_refusal="$transport_bin/expected.stderr"
  printf 'heyfood installer: this installer supports heyfood %s; requested %s\n' \
    "$version" "$installer_request_version" >"$expected_refusal"

  installer_status=0
  PATH="$transport_bin:$PATH" \
    HEYFOOD_VERSION="$installer_request_version" \
    HEYFOOD_CANDIDATE_TRANSPORT_DIRECTORY="$release_directory" \
    HEYFOOD_CANDIDATE_TRANSPORT_VERSION="$version" \
    HEYFOOD_CANDIDATE_TRANSPORT_MANIFEST_SHA256="$approved_manifest_sha256" \
    HEYFOOD_CANDIDATE_TRANSPORT_EXPECT_NO_DOWNLOAD=1 \
    HEYFOOD_CANDIDATE_TRANSPORT_INVOCATION_MARKER="$invocation_marker" \
    /bin/bash "$installer" >"$refusal_stdout" 2>"$refusal_stderr" ||
    installer_status=$?

  [[ ! -e "$invocation_marker" && ! -L "$invocation_marker" ]] ||
    fail "the installer attempted a download in expect-no-download mode"
  [[ "$installer_status" -eq 1 ]] ||
    fail "the installer did not exit 1 at its supported-version refusal"
  [[ ! -s "$refusal_stdout" ]] ||
    fail "the installer wrote stdout before its supported-version refusal"
  cmp -s "$expected_refusal" "$refusal_stderr" ||
    fail "the installer did not emit the exact supported-version refusal"

  printf 'heyfood installer: this installer supports heyfood %s; requested %s\n' \
    "$version" "$installer_request_version" >&2
  exit 1
fi

PATH="$transport_bin:$PATH" \
  HEYFOOD_VERSION="$installer_request_version" \
  HEYFOOD_CANDIDATE_TRANSPORT_DIRECTORY="$release_directory" \
  HEYFOOD_CANDIDATE_TRANSPORT_VERSION="$version" \
  HEYFOOD_CANDIDATE_TRANSPORT_MANIFEST_SHA256="$approved_manifest_sha256" \
  HEYFOOD_CANDIDATE_TRANSPORT_EXPECT_NO_DOWNLOAD=0 \
  /bin/bash "$installer"
