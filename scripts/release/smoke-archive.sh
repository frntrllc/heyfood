#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 || $# -gt 4 ]]; then
  echo "usage: smoke-archive.sh RELEASE_DIRECTORY VERSION TARGET [--complete-set-verified]" >&2
  exit 64
fi

release_directory=$1
version=$2
target=$3
complete_set_verified=${4:-}
if [[ -n "$complete_set_verified" && "$complete_set_verified" != "--complete-set-verified" ]]; then
  echo "unsupported smoke mode: $complete_set_verified" >&2
  exit 64
fi
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
test -d "$release_directory"

case "$target" in
  aarch64-apple-darwin | x86_64-apple-darwin | aarch64-unknown-linux-gnu | x86_64-unknown-linux-gnu) ;;
  *)
    echo "unsupported smoke target: $target" >&2
    exit 64
    ;;
esac

archive_name="heyfood-v$version-$target.tar.gz"
archive="$release_directory/$archive_name"
test -f "$archive"

if [[ -z "$complete_set_verified" ]]; then
  shopt -s dotglob nullglob
  candidate_assets=("$release_directory"/*)
  expected_asset_count=1
  if [[ -f "$release_directory/SHA256SUMS" ]]; then
    expected_asset_count=2
  fi
  test "${#candidate_assets[@]}" -eq "$expected_asset_count"
  for asset_path in "${candidate_assets[@]}"; do
    test -f "$asset_path"
    case "$(basename "$asset_path")" in
      "$archive_name" | SHA256SUMS) ;;
      *)
        echo "unexpected per-target candidate asset: $(basename "$asset_path")" >&2
        exit 1
        ;;
    esac
  done
fi

gzip -t "$archive"
test "$(tar -tzf "$archive")" = "heyfood"
case "$(tar -tvzf "$archive" | cut -c 1)" in
  -) ;;
  *)
    echo "$archive_name must contain one regular executable" >&2
    exit 1
    ;;
esac

if [[ -f "$release_directory/SHA256SUMS" ]]; then
  expected_digest=$(shasum -a 256 "$archive" | awk '{print $1}')
  expected_manifest_line="$expected_digest  $archive_name"
  if [[ -n "$complete_set_verified" ]]; then
    grep -Fqx "$expected_manifest_line" "$release_directory/SHA256SUMS"
  else
    test "$(cat "$release_directory/SHA256SUMS")" = "$expected_manifest_line"
  fi
fi

staging=$(mktemp -d "${TMPDIR:-/tmp}/heyfood-smoke.XXXXXX")
trap 'rm -rf "$staging"' EXIT
tar -xzf "$archive" -C "$staging"
binary="$staging/heyfood"
test -f "$binary"
test -x "$binary"
if [[ "$target" == *-apple-darwin ]]; then
  if [[ -z "${HEYFOOD_APPLE_TEAM_ID:-}" ]]; then
    echo "expected Apple developer team is required for macOS smoke" >&2
    exit 78
  fi
  codesign --verify --deep --strict --verbose=2 "$binary"
  observed_team_id=$(
    codesign --display --verbose=4 "$binary" 2>&1 |
      sed -n 's/^TeamIdentifier=//p'
  )
  if [[ "$observed_team_id" != "$HEYFOOD_APPLE_TEAM_ID" ]]; then
    echo "installed macOS executable is not signed by the expected Apple developer team" >&2
    exit 78
  fi
  spctl --assess --type execute --verbose=2 "$binary"
fi
test "$("$binary" --version)" = "heyfood $version"
"$binary" --help >/dev/null
"$binary" register --help >/dev/null
"$binary" completion bash >"$staging/completion.bash"
test -s "$staging/completion.bash"
