#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: verify-assets.sh RELEASE_DIRECTORY VERSION" >&2
  exit 64
fi

release_directory=$1
version=$2
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
test -d "$release_directory"

expected_archives=(
  "heyfood-v$version-aarch64-apple-darwin.tar.gz"
  "heyfood-v$version-aarch64-unknown-linux-gnu.tar.gz"
  "heyfood-v$version-x86_64-apple-darwin.tar.gz"
  "heyfood-v$version-x86_64-unknown-linux-gnu.tar.gz"
)

asset_count=0
for asset_path in "$release_directory"/*; do
  test -f "$asset_path"
  asset=$(basename "$asset_path")
  case "$asset" in
    SHA256SUMS | "heyfood-v$version-aarch64-apple-darwin.tar.gz" | "heyfood-v$version-aarch64-unknown-linux-gnu.tar.gz" | "heyfood-v$version-x86_64-apple-darwin.tar.gz" | "heyfood-v$version-x86_64-unknown-linux-gnu.tar.gz") ;;
    *)
      echo "unexpected release asset: $asset" >&2
      exit 1
      ;;
  esac
  asset_count=$((asset_count + 1))
done
test "$asset_count" -eq 5

expected_manifest=$(mktemp "${TMPDIR:-/tmp}/heyfood-manifest.XXXXXX")
trap 'rm -f "$expected_manifest"' EXIT
(
  cd "$release_directory"
  LC_ALL=C shasum -a 256 "${expected_archives[@]}" >"$expected_manifest"
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
