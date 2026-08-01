#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "usage: checksums.sh RELEASE_DIRECTORY VERSION [--native-state]" >&2
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

for archive in "${expected_archives[@]}"; do
  test -f "$release_directory/$archive"
done

if [[ "$native_state_mode" == "--native-state" ]]; then
  expected_installer_archives=(
    "heyfood-installer-v$version-aarch64-apple-darwin.tar.gz"
    "heyfood-installer-v$version-aarch64-unknown-linux-gnu.tar.gz"
    "heyfood-installer-v$version-x86_64-apple-darwin.tar.gz"
    "heyfood-installer-v$version-x86_64-unknown-linux-gnu.tar.gz"
  )
  native_state_declaration="heyfood-v$version-native-state.json"
  for archive in "${expected_installer_archives[@]}"; do
    test -f "$release_directory/$archive"
  done
  script_directory=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  /bin/bash "$script_directory/native-state-declaration.sh" "$version" \
    >"$release_directory/$native_state_declaration.tmp"
  mv "$release_directory/$native_state_declaration.tmp" \
    "$release_directory/$native_state_declaration"
  expected_assets+=("${expected_installer_archives[@]}" "$native_state_declaration")
fi

for asset_path in "$release_directory"/*; do
  test -f "$asset_path"
  asset=$(basename "$asset_path")
  [[ "$asset" == "SHA256SUMS" ]] && continue
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
done

(
  cd "$release_directory"
  LC_ALL=C shasum -a 256 "${expected_assets[@]}" >SHA256SUMS.tmp
  mv SHA256SUMS.tmp SHA256SUMS
)
