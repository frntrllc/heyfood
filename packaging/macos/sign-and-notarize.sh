#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: sign-and-notarize.sh BINARY" >&2
  exit 64
fi

binary=$1
test -f "$binary"
test -x "$binary"

required_environment=(
  HEYFOOD_MACOS_CERTIFICATE_P12_BASE64
  HEYFOOD_MACOS_CERTIFICATE_PASSWORD
  HEYFOOD_APPLE_ID
  HEYFOOD_APPLE_APP_PASSWORD
  HEYFOOD_APPLE_TEAM_ID
)
for name in "${required_environment[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    echo "missing protected macOS release input: $name" >&2
    exit 78
  fi
done

signing_directory=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/heyfood-signing.XXXXXX")
keychain="$signing_directory/release.keychain-db"
p12="$signing_directory/developer-id.p12"
submission="$signing_directory/heyfood-notarization.zip"
notary_result="$signing_directory/notary-result.json"
keychain_password=$(openssl rand -hex 32)
keychain_created=false
keychain_search_list_changed=false
original_keychains=()

cleanup() {
  local status=$?
  local cleanup_failed=false

  if [[ "$keychain_search_list_changed" == true ]]; then
    if ! security list-keychains -d user -s "${original_keychains[@]}"; then
      echo "failed to restore the macOS user keychain search list" >&2
      cleanup_failed=true
    fi
  fi
  if [[ "$keychain_created" == true ]]; then
    if ! security delete-keychain "$keychain"; then
      echo "failed to delete the temporary macOS signing keychain" >&2
      cleanup_failed=true
    fi
  fi
  rm -rf -- "$signing_directory"
  if [[ "$cleanup_failed" == true ]]; then
    return 1
  fi
  return "$status"
}
trap cleanup EXIT

while IFS= read -r existing_keychain; do
  if [[ -n "$existing_keychain" ]]; then
    original_keychains+=("$existing_keychain")
  fi
done < <(
  security list-keychains -d user |
    sed -e 's/^[[:space:]]*"//' -e 's/"[[:space:]]*$//'
)

printf '%s' "$HEYFOOD_MACOS_CERTIFICATE_P12_BASE64" |
  openssl base64 -d -A >"$p12"
chmod 0600 "$p12"

security create-keychain -p "$keychain_password" "$keychain"
keychain_created=true
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$p12" \
  -k "$keychain" \
  -P "$HEYFOOD_MACOS_CERTIFICATE_PASSWORD" \
  -T /usr/bin/codesign \
  -T /usr/bin/security
security set-key-partition-list \
  -S apple-tool:,apple: \
  -k "$keychain_password" \
  "$keychain"
keychain_search_list_changed=true
security list-keychains -d user -s "$keychain" "${original_keychains[@]}"

identities=$(security find-identity -v -p codesigning "$keychain" |
  awk '/"Developer ID Application:/{print $2}')
identity_count=$(printf '%s\n' "$identities" | awk 'NF { count += 1 } END { print count + 0 }')
if [[ "$identity_count" -ne 1 ]]; then
  echo "expected exactly one Developer ID Application identity" >&2
  exit 78
fi
identity=$(printf '%s\n' "$identities" | awk 'NF { print; exit }')

codesign \
  --force \
  --options runtime \
  --timestamp \
  --sign "$identity" \
  --keychain "$keychain" \
  "$binary"
codesign --verify --deep --strict --verbose=2 "$binary"
observed_team_id=$(
  codesign --display --verbose=4 "$binary" 2>&1 |
    sed -n 's/^TeamIdentifier=//p'
)
if [[ "$observed_team_id" != "$HEYFOOD_APPLE_TEAM_ID" ]]; then
  echo "signed executable does not match the protected Apple developer team" >&2
  exit 78
fi

ditto -c -k --keepParent "$binary" "$submission"
xcrun notarytool submit "$submission" \
  --apple-id "$HEYFOOD_APPLE_ID" \
  --password "$HEYFOOD_APPLE_APP_PASSWORD" \
  --team-id "$HEYFOOD_APPLE_TEAM_ID" \
  --wait \
  --output-format json >"$notary_result"
python3 - "$notary_result" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    result = json.load(handle)
if result.get("status") != "Accepted":
    raise SystemExit("Apple notarization was not accepted")
submission_id = result.get("id")
if not isinstance(submission_id, str) or not submission_id.strip():
    raise SystemExit("Apple notarization response omitted the submission identifier")
print(
    json.dumps(
        {
            "event": "apple_notarization",
            "status": "Accepted",
            "submission_id": submission_id,
        },
        separators=(",", ":"),
        sort_keys=True,
    )
)
PY

codesign -vvvv -R="notarized" --check-notarization "$binary"
"$binary" --version >/dev/null
