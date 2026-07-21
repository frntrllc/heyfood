#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: package.sh BINARY VERSION TARGET OUTPUT_DIRECTORY" >&2
  exit 64
fi

binary=$1
version=$2
target=$3
output_directory=$4

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
case "$target" in
  aarch64-apple-darwin | x86_64-apple-darwin | aarch64-unknown-linux-gnu | x86_64-unknown-linux-gnu) ;;
  *)
    echo "unsupported release target: $target" >&2
    exit 64
    ;;
esac
test -f "$binary"
test -x "$binary"

tar_command=tar
if command -v gtar >/dev/null 2>&1; then
  tar_command=gtar
fi
if ! "$tar_command" --version 2>/dev/null | grep -q 'GNU tar'; then
  echo "GNU tar is required to produce normalized release archives" >&2
  exit 69
fi

mkdir -p "$output_directory"
archive="$output_directory/heyfood-v$version-$target.tar.gz"
staging=$(mktemp -d "${TMPDIR:-/tmp}/heyfood-package.XXXXXX")
trap 'rm -rf "$staging"' EXIT

install -m 0755 "$binary" "$staging/heyfood"
LC_ALL=C "$tar_command" \
  --sort=name \
  --format=ustar \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  --mtime='UTC 1980-01-01' \
  -C "$staging" \
  -cf - heyfood | gzip -n >"$archive.tmp"
mv "$archive.tmp" "$archive"
gzip -t "$archive"
test "$("$tar_command" -tzf "$archive")" = "heyfood"
