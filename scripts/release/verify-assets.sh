#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "usage: verify-assets.sh RELEASE_DIRECTORY VERSION [--native-state]" >&2
  exit 64
fi

release_directory=$1
version=$2
native_state_mode=${3:-}
if [[ -n "$native_state_mode" && "$native_state_mode" != "--native-state" ]]; then
  echo "unsupported release mode: $native_state_mode" >&2
  exit 64
fi
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
test -d "$release_directory"

expected_archives=(
  "heyfood-v$version-aarch64-apple-darwin.tar.gz"
  "heyfood-v$version-aarch64-unknown-linux-gnu.tar.gz"
  "heyfood-v$version-x86_64-apple-darwin.tar.gz"
  "heyfood-v$version-x86_64-unknown-linux-gnu.tar.gz"
)
expected_assets=("${expected_archives[@]}")
expected_installer_archives=()
native_state_declaration=""
if [[ "$native_state_mode" == "--native-state" ]]; then
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
for asset_path in "$release_directory"/*; do
  test -f "$asset_path"
  asset=$(basename "$asset_path")
  if [[ "$asset" != "SHA256SUMS" ]]; then
    found=false
    for expected in "${expected_assets[@]}"; do
      if [[ "$asset" == "$expected" ]]; then
        found=true
        break
      fi
    done
    if [[ "$found" != "true" ]]; then
      echo "unexpected release asset: $asset" >&2
      exit 1
    fi
  fi
  asset_count=$((asset_count + 1))
done
test "$asset_count" -eq "$((${#expected_assets[@]} + 1))"

expected_manifest=$(mktemp "${TMPDIR:-/tmp}/heyfood-manifest.XXXXXX")
expected_native_state=""
if [[ -n "$native_state_declaration" ]]; then
  test -f "$release_directory/$native_state_declaration"
  expected_native_state=$(mktemp "${TMPDIR:-/tmp}/heyfood-native-state.XXXXXX")
  script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  /bin/bash "$script_directory/native-state-declaration.sh" "$version" >"$expected_native_state"
  cmp "$expected_native_state" "$release_directory/$native_state_declaration"
fi
cleanup() {
  rm -f "$expected_manifest"
  if [[ -n "$expected_native_state" ]]; then
    rm -f "$expected_native_state"
  fi
}
trap cleanup EXIT

(
  cd "$release_directory"
  LC_ALL=C shasum -a 256 "${expected_assets[@]}" >"$expected_manifest"
)
cmp "$expected_manifest" "$release_directory/SHA256SUMS"

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

if [[ "$native_state_mode" == "--native-state" ]]; then
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
