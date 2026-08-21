#!/usr/bin/env bash

set -euo pipefail

usage() {
  printf 'usage: %s <binary> <product|verifier> <linux-target> [evidence.json]\n' "$0" >&2
  exit 64
}

[[ $# -ge 3 && $# -le 4 ]] || usage

binary=$1
kind=$2
target=$3
evidence=${4:-}
readonly binary kind target evidence
readonly maximum_glibc="2.34"

[[ -f "$binary" && -x "$binary" ]] || {
  printf 'Linux ELF verification requires an executable file: %s\n' "$binary" >&2
  exit 66
}
command -v readelf >/dev/null 2>&1 || {
  printf 'Linux ELF verification requires readelf\n' >&2
  exit 69
}
command -v jq >/dev/null 2>&1 || {
  printf 'Linux ELF verification requires jq\n' >&2
  exit 69
}

case "$kind" in
  product)
    expected_needed_text=$'libc.so.6\nlibdbus-1.so.3\nlibgcc_s.so.1\nlibm.so.6'
    ;;
  verifier)
    expected_needed_text=$'libc.so.6\nlibgcc_s.so.1'
    ;;
  *) usage ;;
esac
readonly expected_needed_text

case "$target" in
  x86_64-unknown-linux-gnu)
    expected_machine="Advanced Micro Devices X86-64"
    expected_interpreter="/lib64/ld-linux-x86-64.so.2"
    ;;
  aarch64-unknown-linux-gnu)
    expected_machine="AArch64"
    expected_interpreter="/lib/ld-linux-aarch64.so.1"
    ;;
  *)
    printf 'unsupported Linux ELF target: %s\n' "$target" >&2
    exit 64
    ;;
esac
readonly expected_machine expected_interpreter

machine=$(readelf -h -- "$binary" | sed -n 's/^[[:space:]]*Machine:[[:space:]]*//p')
interpreter=$(readelf -l -- "$binary" |
  sed -n 's/.*Requesting program interpreter: \([^]]*\)].*/\1/p')
observed_needed=$(
  readelf -d -- "$binary" |
    sed -n 's/.*Shared library: \[\([^]]*\)\].*/\1/p' |
    LC_ALL=C sort -u
)
glibc_versions=$(
  readelf --version-info -- "$binary" |
    grep -oE 'GLIBC_[0-9]+(\.[0-9]+)*' |
    sed 's/^GLIBC_//' |
    sort -Vu
)
readonly observed_needed glibc_versions

[[ "$machine" == "$expected_machine" ]] || {
  printf 'unexpected ELF machine for %s: %s (expected %s)\n' \
    "$target" "$machine" "$expected_machine" >&2
  exit 1
}
[[ "$interpreter" == "$expected_interpreter" ]] || {
  printf 'unexpected ELF interpreter for %s: %s (expected %s)\n' \
    "$target" "$interpreter" "$expected_interpreter" >&2
  exit 1
}
[[ -n "$glibc_versions" ]] || {
  printf 'no GLIBC symbol versions found in %s\n' "$binary" >&2
  exit 1
}
observed_glibc=$(printf '%s\n' "$glibc_versions" | tail -1)
readonly observed_glibc
highest=$(printf '%s\n%s\n' "$observed_glibc" "$maximum_glibc" | sort -V | tail -1)
[[ "$highest" == "$maximum_glibc" ]] || {
  printf 'GLIBC_%s exceeds the supported GLIBC_%s floor in %s\n' \
    "$observed_glibc" "$maximum_glibc" "$binary" >&2
  exit 1
}

[[ "$observed_needed" == "$expected_needed_text" ]] || {
  printf 'unexpected DT_NEEDED boundary for %s (%s)\nexpected:\n%s\nobserved:\n%s\n' \
    "$binary" "$kind" "$expected_needed_text" "$observed_needed" >&2
  exit 1
}

report=$(jq -n \
  --arg schema_version "1" \
  --arg binary "$(basename -- "$binary")" \
  --arg kind "$kind" \
  --arg target "$target" \
  --arg machine "$machine" \
  --arg interpreter "$interpreter" \
  --arg maximum_glibc "$maximum_glibc" \
  --arg observed_glibc "$observed_glibc" \
  --arg needed "$observed_needed" \
  '{
    schema_version: ($schema_version | tonumber),
    binary: $binary,
    kind: $kind,
    target: $target,
    machine: $machine,
    interpreter: $interpreter,
    maximum_supported_glibc: $maximum_glibc,
    maximum_observed_glibc: $observed_glibc,
    needed: ($needed | split("\n") | map(select(length > 0)))
  }')

if [[ -n "$evidence" ]]; then
  mkdir -p -- "$(dirname -- "$evidence")"
  printf '%s\n' "$report" >"$evidence"
else
  printf '%s\n' "$report"
fi
