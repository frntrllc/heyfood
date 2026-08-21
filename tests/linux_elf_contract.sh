#!/usr/bin/env bash

set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT
readonly VERIFIER="$ROOT/scripts/release/verify-linux-elf.sh"
CASE_DIR=$(mktemp -d)
readonly CASE_DIR

cleanup() {
  rm -rf -- "$CASE_DIR"
}
trap cleanup EXIT

fail() {
  printf 'Linux ELF contract: %s\n' "$*" >&2
  exit 1
}

mkdir -p "$CASE_DIR/bin"
touch "$CASE_DIR/heyfood"
chmod 700 "$CASE_DIR/heyfood"

cat >"$CASE_DIR/bin/readelf" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  -h)
    printf '  Machine:                           %s\n' "${MOCK_MACHINE:-Advanced Micro Devices X86-64}"
    ;;
  -l)
    printf '      [Requesting program interpreter: %s]\n' "${MOCK_INTERPRETER:-/lib64/ld-linux-x86-64.so.2}"
    ;;
  -d)
    if [[ "${MOCK_KIND:-product}" == "product" ]]; then
      printf ' 0x0000000000000001 (NEEDED) Shared library: [%s]\n' \
        "${MOCK_LOADER_NEEDED:-ld-linux-x86-64.so.2}"
      for library in libc.so.6 libdbus-1.so.3 libgcc_s.so.1 libm.so.6; do
        printf ' 0x0000000000000001 (NEEDED) Shared library: [%s]\n' "$library"
      done
    else
      for library in libc.so.6 libgcc_s.so.1; do
        printf ' 0x0000000000000001 (NEEDED) Shared library: [%s]\n' "$library"
      done
    fi
    if [[ -n "${MOCK_EXTRA_NEEDED:-}" ]]; then
      printf ' 0x0000000000000001 (NEEDED) Shared library: [%s]\n' "$MOCK_EXTRA_NEEDED"
    fi
    ;;
  --version-info)
    printf '  Name: GLIBC_2.17\n  Name: GLIBC_%s\n' "${MOCK_GLIBC:-2.34}"
    ;;
  *) exit 64 ;;
esac
MOCK
chmod 700 "$CASE_DIR/bin/readelf"

PATH="$CASE_DIR/bin:$PATH" "$VERIFIER" \
  "$CASE_DIR/heyfood" product x86_64-unknown-linux-gnu "$CASE_DIR/product.json"
jq -e '
  .schema_version == 1 and
  .kind == "product" and
  .target == "x86_64-unknown-linux-gnu" and
  .maximum_supported_glibc == "2.34" and
  .maximum_observed_glibc == "2.34" and
  .needed == [
    "ld-linux-x86-64.so.2",
    "libc.so.6",
    "libdbus-1.so.3",
    "libgcc_s.so.1",
    "libm.so.6"
  ]
' "$CASE_DIR/product.json" >/dev/null || fail "product evidence is incomplete"

MOCK_KIND=verifier PATH="$CASE_DIR/bin:$PATH" "$VERIFIER" \
  "$CASE_DIR/heyfood" verifier x86_64-unknown-linux-gnu "$CASE_DIR/verifier.json"
jq -e '.needed == ["libc.so.6", "libgcc_s.so.1"]' \
  "$CASE_DIR/verifier.json" >/dev/null || fail "verifier boundary is incomplete"

if MOCK_GLIBC=2.35 PATH="$CASE_DIR/bin:$PATH" "$VERIFIER" \
  "$CASE_DIR/heyfood" product x86_64-unknown-linux-gnu >/dev/null 2>&1; then
  fail "GLIBC versions above 2.34 must fail"
fi

if MOCK_EXTRA_NEEDED=libssl.so.3 PATH="$CASE_DIR/bin:$PATH" "$VERIFIER" \
  "$CASE_DIR/heyfood" product x86_64-unknown-linux-gnu >/dev/null 2>&1; then
  fail "unexpected native dependencies must fail"
fi

if PATH="$CASE_DIR/bin:$PATH" "$VERIFIER" \
  "$CASE_DIR/heyfood" product x86_64-unknown-linux-musl >/dev/null 2>&1; then
  fail "unqualified Linux targets must fail"
fi

grep -Fq 'maximum_glibc="2.34"' "$VERIFIER" ||
  fail "the glibc ceiling must remain explicit"
grep -Fq "base_expected_needed=\$'libc.so.6\\nlibdbus-1.so.3\\nlibgcc_s.so.1\\nlibm.so.6'" \
  "$VERIFIER" || fail "the product DT_NEEDED allowlist must remain explicit"
