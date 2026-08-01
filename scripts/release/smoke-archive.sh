#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 || $# -gt 5 ]]; then
  echo "usage: smoke-archive.sh RELEASE_DIRECTORY VERSION TARGET [--complete-set-verified] [--native-state]" >&2
  exit 64
fi

release_directory=$1
version=$2
target=$3
shift 3
complete_set_verified=""
native_state_release=false
for mode in "$@"; do
  case "$mode" in
    --complete-set-verified)
      [[ -z "$complete_set_verified" ]] || {
        echo "duplicate smoke mode: $mode" >&2
        exit 64
      }
      complete_set_verified="$mode"
      ;;
    --native-state)
      [[ "$native_state_release" == "false" ]] || {
        echo "duplicate smoke mode: $mode" >&2
        exit 64
      }
      native_state_release=true
      ;;
    *)
      echo "unsupported smoke mode: $mode" >&2
      exit 64
      ;;
  esac
done
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
installer_archive_name=""
installer_archive=""
if [[ "$native_state_release" == "true" ]]; then
  installer_archive_name="heyfood-installer-v$version-$target.tar.gz"
  installer_archive="$release_directory/$installer_archive_name"
  test -f "$installer_archive"
fi

if [[ -z "$complete_set_verified" ]]; then
  shopt -s dotglob nullglob
  candidate_assets=("$release_directory"/*)
  expected_asset_count=1
  if [[ "$native_state_release" == "true" ]]; then
    expected_asset_count=2
  fi
  if [[ -f "$release_directory/SHA256SUMS" ]]; then
    expected_asset_count=$((expected_asset_count + 1))
  fi
  test "${#candidate_assets[@]}" -eq "$expected_asset_count"
  for asset_path in "${candidate_assets[@]}"; do
    test -f "$asset_path"
    case "$(basename "$asset_path")" in
      "$archive_name" | "$installer_archive_name" | SHA256SUMS) ;;
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

if [[ "$native_state_release" == "true" ]]; then
  gzip -t "$installer_archive"
  test "$(tar -tzf "$installer_archive")" = "heyfood-installer"
  case "$(tar -tvzf "$installer_archive" | cut -c 1)" in
    -) ;;
    *)
      echo "$installer_archive_name must contain one regular executable" >&2
      exit 1
      ;;
  esac
fi

if [[ -f "$release_directory/SHA256SUMS" ]]; then
  expected_digest=$(shasum -a 256 "$archive" | awk '{print $1}')
  expected_manifest_line="$expected_digest  $archive_name"
  if [[ -n "$complete_set_verified" || "$native_state_release" == "true" ]]; then
    grep -Fqx "$expected_manifest_line" "$release_directory/SHA256SUMS"
  else
    test "$(cat "$release_directory/SHA256SUMS")" = "$expected_manifest_line"
  fi
  if [[ "$native_state_release" == "true" ]]; then
    expected_installer_digest=$(shasum -a 256 "$installer_archive" | awk '{print $1}')
    expected_installer_manifest_line="$expected_installer_digest  $installer_archive_name"
    grep -Fqx "$expected_installer_manifest_line" "$release_directory/SHA256SUMS"
  fi
fi

staging=$(mktemp -d "${TMPDIR:-/tmp}/heyfood-smoke.XXXXXX")
trap 'rm -rf "$staging"' EXIT
mkdir "$staging/product"
tar -xzf "$archive" -C "$staging/product"
binary="$staging/product/heyfood"
test -f "$binary"
test -x "$binary"
verifier=""
if [[ "$native_state_release" == "true" ]]; then
  mkdir "$staging/installer"
  tar -xzf "$installer_archive" -C "$staging/installer"
  verifier="$staging/installer/heyfood-installer"
  test -f "$verifier"
  test -x "$verifier"
fi
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
  codesign -vvvv -R="notarized" --check-notarization "$binary"
  if [[ "$native_state_release" == "true" ]]; then
    codesign --verify --deep --strict --verbose=2 "$verifier"
    verifier_team_id=$(
      codesign --display --verbose=4 "$verifier" 2>&1 |
        sed -n 's/^TeamIdentifier=//p'
    )
    if [[ "$verifier_team_id" != "$HEYFOOD_APPLE_TEAM_ID" ]]; then
      echo "packaged installer verifier is not signed by the expected Apple developer team" >&2
      exit 78
    fi
    codesign -vvvv -R="notarized" --check-notarization "$verifier"
  fi
fi
test "$("$binary" --version)" = "heyfood $version"
"$binary" --help >/dev/null
"$binary" register --help >/dev/null
"$binary" completion bash >"$staging/completion.bash"
test -s "$staging/completion.bash"

"$binary" agent describe >"$staging/agent-manifest.json"
jq -e \
  '(.schema_version == 1 or .schema_version == 2)
   and .automation_surfaces.mcp_stdio == "active"
   and ([.commands[].path] | index("mcp serve")) != null
   and ([.capabilities[] | select(.id == "agent-mcp" and .status == "active")] | length) == 1' \
  "$staging/agent-manifest.json" >/dev/null
if [[ "$native_state_release" == "true" ]]; then
  test "$("$verifier" --version)" = "heyfood-installer $version"
  declaration="$release_directory/heyfood-v$version-native-state.json"
  if [[ ! -f "$declaration" ]]; then
    declaration="$staging/heyfood-v$version-native-state.json"
    scripts/release/native-state-declaration.sh "$version" >"$declaration"
  fi
  "$verifier" verify-native-state \
    "$version" \
    - \
    - \
    "$declaration" \
    "$staging/agent-manifest.json"
fi
"$binary" agent guide --format markdown >"$staging/agent-guide.md"
grep -Fq 'heyfood mcp serve' "$staging/agent-guide.md"

if env -i \
  HOME="$HOME" \
  PATH="$PATH" \
  HEYFOOD_UNKNOWN_QUALIFICATION_OVERRIDE=must-not-be-read \
  "$binary" mcp serve >"$staging/mcp.stdout" 2>"$staging/mcp.stderr"; then
  echo "MCP accepted a forbidden inherited HEYFOOD_* variable" >&2
  exit 1
fi
test ! -s "$staging/mcp.stdout"
grep -Fq 'HEYFOOD_UNKNOWN_QUALIFICATION_OVERRIDE' "$staging/mcp.stderr"
if env -i HOME="$HOME" PATH="$PATH" \
  "$binary" --json mcp serve >"$staging/mcp-modifier.stdout" 2>"$staging/mcp-modifier.stderr"; then
  echo "MCP accepted a one-shot output modifier" >&2
  exit 1
fi
test ! -s "$staging/mcp-modifier.stdout"
grep -Fq 'stdout is reserved for MCP' "$staging/mcp-modifier.stderr"
node scripts/release/mcp-smoke.mjs "$binary"
scripts/release/agent-setup-smoke.sh "$binary" "$staging/agent-setup"
