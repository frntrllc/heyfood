#!/usr/bin/env bash
set -euo pipefail

fail() {
  printf 'verify assets: %s\n' "$*" >&2
  exit 1
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    fail "sha256sum or shasum is required"
  fi
}

if [[ $# -lt 2 || $# -gt 4 ]]; then
  echo "usage: verify-assets.sh RELEASE_DIRECTORY VERSION [--native-state | --native-state-manifest-bound SHA256SUMS_SHA256]" >&2
  exit 64
fi

release_directory=$1
version=$2
release_mode=${3:-}
approved_manifest_sha256=${4:-}
native_state_mode=false
manifest_bound_mode=false
case "$release_mode" in
  "")
    [[ $# -eq 2 ]] || fail "the default release mode takes no additional argument"
    ;;
  --native-state)
    [[ $# -eq 3 ]] || fail "--native-state takes no additional argument"
    native_state_mode=true
    ;;
  --native-state-manifest-bound)
    [[ $# -eq 4 ]] || fail "--native-state-manifest-bound requires SHA256SUMS_SHA256"
    [[ "$approved_manifest_sha256" =~ ^[0-9a-f]{64}$ ]] ||
      fail "SHA256SUMS_SHA256 must be a lowercase SHA-256 digest"
    native_state_mode=true
    manifest_bound_mode=true
    ;;
  *)
    echo "unsupported release mode: $release_mode" >&2
    exit 64
    ;;
esac
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  fail "VERSION must be an exact release version"
[[ -d "$release_directory" && ! -L "$release_directory" ]] ||
  fail "RELEASE_DIRECTORY must be a real directory"

expected_archives=(
  "heyfood-v$version-aarch64-apple-darwin.tar.gz"
  "heyfood-v$version-aarch64-unknown-linux-gnu.tar.gz"
  "heyfood-v$version-x86_64-apple-darwin.tar.gz"
  "heyfood-v$version-x86_64-unknown-linux-gnu.tar.gz"
)
expected_assets=("${expected_archives[@]}")
expected_installer_archives=()
native_state_declaration=""
if [[ "$native_state_mode" == true ]]; then
  expected_installer_archives=(
    "heyfood-installer-v$version-aarch64-apple-darwin.tar.gz"
    "heyfood-installer-v$version-aarch64-unknown-linux-gnu.tar.gz"
    "heyfood-installer-v$version-x86_64-apple-darwin.tar.gz"
    "heyfood-installer-v$version-x86_64-unknown-linux-gnu.tar.gz"
  )
  native_state_declaration="heyfood-v$version-native-state.json"
  expected_assets+=("${expected_installer_archives[@]}" "$native_state_declaration")
fi

asset_count=0
while IFS= read -r -d '' asset_path; do
  [[ -f "$asset_path" && ! -L "$asset_path" ]] ||
    fail "the release set contains a non-regular asset"
  asset=${asset_path##*/}
  if [[ "$asset" != "SHA256SUMS" ]]; then
    found=false
    for expected in "${expected_assets[@]}"; do
      if [[ "$asset" == "$expected" ]]; then
        found=true
        break
      fi
    done
    if [[ "$found" != "true" ]]; then
      fail "unexpected release asset: $asset"
    fi
  fi
  asset_count=$((asset_count + 1))
done < <(find "$release_directory" -mindepth 1 -maxdepth 1 -print0)
[[ "$asset_count" -eq "$((${#expected_assets[@]} + 1))" ]] ||
  fail "the release set does not contain the exact expected file count"
for expected in "${expected_assets[@]}"; do
  [[ -f "$release_directory/$expected" && ! -L "$release_directory/$expected" ]] ||
    fail "missing expected release asset: $expected"
done
manifest="$release_directory/SHA256SUMS"
[[ -f "$manifest" && ! -L "$manifest" ]] ||
  fail "SHA256SUMS must be a regular file"

expected_manifest=$(mktemp "${TMPDIR:-/tmp}/heyfood-manifest.XXXXXX")
expected_native_state=""
cleanup() {
  rm -f "$expected_manifest"
  if [[ -n "$expected_native_state" ]]; then
    rm -f "$expected_native_state"
  fi
}
trap cleanup EXIT

if [[ "$manifest_bound_mode" == true ]]; then
  [[ "$(sha256_file "$manifest")" == "$approved_manifest_sha256" ]] ||
    fail "SHA256SUMS does not match the approved digest"
elif [[ -n "$native_state_declaration" ]]; then
  expected_native_state=$(mktemp "${TMPDIR:-/tmp}/heyfood-native-state.XXXXXX")
  script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  /bin/bash "$script_directory/native-state-declaration.sh" "$version" >"$expected_native_state"
  cmp -s "$expected_native_state" "$release_directory/$native_state_declaration" ||
    fail "the native-state declaration is not canonical"
fi

: >"$expected_manifest"
for expected in "${expected_assets[@]}"; do
  digest=$(sha256_file "$release_directory/$expected")
  [[ "$digest" =~ ^[0-9a-f]{64}$ ]] ||
    fail "could not calculate a lowercase SHA-256 for $expected"
  printf '%s  %s\n' "$digest" "$expected" >>"$expected_manifest"
done

manifest_entry_count=$(wc -l <"$manifest" | tr -d '[:space:]')
[[ "$manifest_entry_count" -eq "${#expected_assets[@]}" ]] ||
  fail "SHA256SUMS must contain exactly ${#expected_assets[@]} entries"
for expected in "${expected_assets[@]}"; do
  entry_count=$(awk -v asset="$expected" 'NF == 2 && $2 == asset { count += 1 } END { print count + 0 }' "$manifest")
  [[ "$entry_count" -eq 1 ]] ||
    fail "SHA256SUMS must bind $expected exactly once"
done
cmp -s "$expected_manifest" "$manifest" ||
  fail "SHA256SUMS does not exactly bind the release assets"

for archive in "${expected_archives[@]}"; do
  archive_path="$release_directory/$archive"
  gzip -t "$archive_path"
  test "$(tar -tzf "$archive_path")" = "heyfood"
  case "$(tar -tvzf "$archive_path" | cut -c 1)" in
    -) ;;
    *)
      echo "$archive must contain one regular executable" >&2
      exit 1
      ;;
  esac
done

if [[ "$native_state_mode" == true ]]; then
  for archive in "${expected_installer_archives[@]}"; do
    archive_path="$release_directory/$archive"
    gzip -t "$archive_path"
    test "$(tar -tzf "$archive_path")" = "heyfood-installer"
    case "$(tar -tvzf "$archive_path" | cut -c 1)" in
      -) ;;
      *)
        echo "$archive must contain one regular executable" >&2
        exit 1
        ;;
    esac
  done
fi
