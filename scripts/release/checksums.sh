#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: checksums.sh RELEASE_DIRECTORY VERSION" >&2
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

for archive in "${expected_archives[@]}"; do
  test -f "$release_directory/$archive"
done

(
  cd "$release_directory"
  LC_ALL=C shasum -a 256 "${expected_archives[@]}" >SHA256SUMS.tmp
  mv SHA256SUMS.tmp SHA256SUMS
)
