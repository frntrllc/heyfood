#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

workflow=.github/workflows/production-tui-canary.yml
runner=scripts/eval/run-production-canary.sh
triage=scripts/eval/triage-production-canary.sh
finalize=scripts/eval/finalize-production-canary.sh
rotate=scripts/eval/rotate-production-canary-state.sh

test -f "$workflow"
test -x "$runner"
test -x "$triage"
test -x "$finalize"
test -x "$rotate"

grep -Fq 'cron: "23 */6 * * *"' "$workflow"
grep -Fq "environment: native-eval" "$workflow"
grep -Fq "github.ref == 'refs/heads/main'" "$workflow"
grep -Fq "github.event_name == 'workflow_dispatch'" "$workflow"
grep -Fq "github.event_name == 'workflow_dispatch' ||" "$workflow"
grep -Fq "HEYFOOD_PRODUCTION_CANARY_ENABLED == 'true'" "$workflow"
grep -Fq "HEYFOOD_CANARY_STATE_BUNDLE_B64" "$workflow"
grep -Fq "HEYFOOD_CANARY_ROTATOR_APP_ID" "$workflow"
grep -Fq "HEYFOOD_CANARY_ROTATOR_APP_PRIVATE_KEY" "$workflow"
grep -Fq "permission-environments: write" "$workflow"
grep -Fq "gh attestation verify" "$workflow"
grep -Fq "sudo apt-get install --yes dbus-x11 gnome-keyring" "$workflow"
grep -Fq "dbus-run-session -- bash -euo pipefail -c" "$workflow"
grep -Fq "gnome-keyring-daemon --unlock" "$workflow"
grep -Fq "scripts/eval/run-production-canary.sh" "$workflow"
grep -Fq "scripts/eval/triage-production-canary.sh" "$workflow"
grep -Fq "scripts/eval/finalize-production-canary.sh" "$workflow"
grep -Fq "scripts/eval/rotate-production-canary-state.sh" "$workflow"
initialize_line=$(grep -nF "name: Initialize fail-safe evidence" "$workflow" | cut -d: -f1)
secret_service_line=$(grep -nF "name: Install the Linux Secret Service runtime" "$workflow" | cut -d: -f1)
test "$initialize_line" -lt "$secret_service_line"
if grep -Eq -- \
  'grocery[[:space:]]+(add|remove|state|never|confirm)|--decision[[:space:]]+(accept|cancel)' \
  "$workflow" "$runner"; then
  echo "automated production canary must never invoke a human-only Grocery command" >&2
  exit 1
fi

scratch=$(mktemp -d "${TMPDIR:-/tmp}/heyfood-production-canary-contract.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
if ! command -v timeout >/dev/null 2>&1; then
  cat >"$scratch/timeout" <<'TIMEOUT'
#!/usr/bin/env bash
set -euo pipefail
shift 3
exec "$@"
TIMEOUT
  chmod 0700 "$scratch/timeout"
  export PATH="$scratch:$PATH"
fi
state="$scratch/state"
evidence="$scratch/evidence"
mkdir -p "$state" "$evidence"
chmod 0700 "$state"
printf '%s\n' fixture-auth >"$state/auth.native"
printf '%s\n' fixture-credentials >"$state/credentials.native"
chmod 0600 "$state/auth.native" "$state/credentials.native"

stub="$scratch/heyfood"
cat >"$stub" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
args=" $* "
if [[ "${HEYFOOD_CANARY_STUB_FAILURE:-}" =~ ^(availability|http_status)$ &&
  "$args" == *" grocery list "* ]]; then
  error_type=request_transport
  if [[ "${HEYFOOD_CANARY_STUB_FAILURE:-}" == http_status ]]; then
    error_type=http_status
  fi
  jq -cn --arg error_type "$error_type" '{
    ok: false,
    error: {type: $error_type, message: "redacted"}
  }'
  exit 1
fi
if [[ "$args" == *" grocery list "* ]]; then
  version=7
  if [[ "${HEYFOOD_CANARY_STUB_FAILURE:-}" == mutation &&
    -f "${HEYFOOD_CANARY_STUB_COUNTER:?}" ]]; then
    version=8
  fi
  : >"${HEYFOOD_CANARY_STUB_COUNTER:?}"
  jq -cn --argjson version "$version" '{
    id: "00000000-0000-4000-8000-000000000030",
    title: "Synthetic canary",
    state: "active",
    version: $version,
    items: [],
    created_at: "2026-07-26T00:00:00Z",
    updated_at: "2026-07-26T00:00:00Z"
  }'
elif [[ "$args" == *" diet list "* ]]; then
  printf '%s\n' '{"corpus_available":true,"diets":[{"id":"synthetic","label":"Synthetic diet","tier":1,"evidence_level":"strong","covered":true,"summary":"fixture"}],"count":1}'
elif [[ "$args" == *" diet show synthetic "* ]]; then
  printf '%s\n' '{"id":"synthetic","label":"Synthetic diet","tier":1,"evidence_level":"strong","covered":true,"detail_status":"covered","summary":"fixture","sections":{"safety":["fixture"]},"citations":[],"contraindicated_conditions":[]}'
elif [[ "$args" == *" ask "* ]]; then
  cat >/dev/null
  printf '%s\n' '{"message":"synthetic acknowledgement"}'
else
  printf '%s\n' '{"ok":false,"error":{"type":"fixture_command","message":"redacted"}}'
  exit 1
fi
STUB
chmod 0700 "$stub"

run_fixture() {
  local failure=$1
  local counter="$scratch/counter-$failure"
  rm -f "$counter"
  HEYFOOD_CANARY_BINARY="$stub" \
    HEYFOOD_CANARY_STATE_DIR="$state" \
    HEYFOOD_CANARY_EVIDENCE_DIR="$evidence-$failure" \
    HEYFOOD_CANARY_VERSION=0.5.0 \
    HEYFOOD_CANARY_TARGET=x86_64-unknown-linux-gnu \
    HEYFOOD_CANARY_ARCHIVE_SHA256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
    HEYFOOD_CANARY_STUB_FAILURE="$failure" \
    HEYFOOD_CANARY_STUB_COUNTER="$counter" \
    "$runner"
}

run_fixture success
success="$evidence-success/production-canary.json"
jq -e '
  .status == "passed" and
  .mutation_policy.proposal_prepared == false and
  .mutation_policy.decision == null and
  .mutation_policy.non_mutation_verified == true and
  .mutation_policy.accept_permitted == false and
  (.operations | length == 5) and
  ([.operations[].id] == ["grocery_read", "agent_turn", "diet_catalog", "diet_detail", "grocery_nonmutation"]) and
  .privacy.raw_requests_retained == false and
  .privacy.raw_responses_retained == false
' "$success" >/dev/null
if grep -Eq 'synthetic acknowledgement|synthetic diet|00000000-0000-4000-8000-000000000030' \
  "$success"; then
  echo "privacy-safe evidence retained private journey content" >&2
  exit 1
fi

if run_fixture availability; then
  echo "availability fixture unexpectedly passed" >&2
  exit 1
fi
jq -e '
  .status == "failed" and
  .failure.category == "availability" and
  .failure.error_type == "request_transport"
' "$evidence-availability/production-canary.json" >/dev/null

if run_fixture http_status; then
  echo "generic HTTP-status fixture unexpectedly passed" >&2
  exit 1
fi
jq -e '
  .status == "failed" and
  .failure.category == "availability" and
  .failure.error_type == "http_status"
' "$evidence-http_status/production-canary.json" >/dev/null

if run_fixture mutation; then
  echo "mutation fixture unexpectedly passed" >&2
  exit 1
fi
jq -e '
  .status == "failed" and
  .failure.category == "safety" and
  .failure.error_type == "list_authority_changed"
' "$evidence-mutation/production-canary.json" >/dev/null

triage_bin="$scratch/triage-bin"
triage_log="$scratch/triage.log"
mkdir "$triage_bin"
cat >"$triage_bin/gh" <<'GH'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1 $2" == "issue list" ]]; then
  exit 0
fi
printf '%s\n' "$1 $2" >>"${HEYFOOD_CANARY_TRIAGE_LOG:?}"
GH
chmod 0700 "$triage_bin/gh"

PATH="$triage_bin:$PATH" \
  HEYFOOD_CANARY_TRIAGE_LOG="$triage_log" \
  "$triage" \
  "$evidence-availability/production-canary.json" \
  - \
  0.5.0 \
  https://example.invalid/first \
  frntrllc/heyfood
test ! -e "$triage_log"

mismatched_previous="$scratch/mismatched-previous.json"
jq '.release.version = "0.5.1"' \
  "$evidence-availability/production-canary.json" >"$mismatched_previous"
PATH="$triage_bin:$PATH" \
  HEYFOOD_CANARY_TRIAGE_LOG="$triage_log" \
  "$triage" \
  "$evidence-availability/production-canary.json" \
  "$mismatched_previous" \
  0.5.0 \
  https://example.invalid/version-bound \
  frntrllc/heyfood
test ! -e "$triage_log"

mismatched_digest="$scratch/mismatched-digest.json"
jq '.release.archive_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"' \
  "$evidence-availability/production-canary.json" >"$mismatched_digest"
PATH="$triage_bin:$PATH" \
  HEYFOOD_CANARY_TRIAGE_LOG="$triage_log" \
  "$triage" \
  "$evidence-availability/production-canary.json" \
  "$mismatched_digest" \
  0.5.0 \
  https://example.invalid/digest-bound \
  frntrllc/heyfood
test ! -e "$triage_log"

PATH="$triage_bin:$PATH" \
  HEYFOOD_CANARY_TRIAGE_LOG="$triage_log" \
  "$triage" \
  "$evidence-availability/production-canary.json" \
  "$evidence-availability/production-canary.json" \
  0.5.0 \
  https://example.invalid/second \
  frntrllc/heyfood
grep -Fqx "issue create" "$triage_log"

: >"$triage_log"
PATH="$triage_bin:$PATH" \
  HEYFOOD_CANARY_TRIAGE_LOG="$triage_log" \
  "$triage" \
  "$evidence-mutation/production-canary.json" \
  - \
  0.5.0 \
  https://example.invalid/safety \
  frntrllc/heyfood
grep -Fqx "issue create" "$triage_log"

: >"$triage_log"
postcondition_failure="$scratch/postcondition-failure.json"
cp "$success" "$postcondition_failure"
"$finalize" "$postcondition_failure" success failure success
jq -e '
  .status == "failed" and
  .failure.category == "credential" and
  .failure.operation == "state_rotation" and
  .failure.error_type == "protected_state_rotation_failed" and
  .postconditions.protected_state_rotation == "failure"
' "$postcondition_failure" >/dev/null
PATH="$triage_bin:$PATH" \
  HEYFOOD_CANARY_TRIAGE_LOG="$triage_log" \
  HEYFOOD_CANARY_JOB_RESULT=failure \
  "$triage" \
  "$postcondition_failure" \
  - \
  0.5.0 \
  https://example.invalid/rotation \
  frntrllc/heyfood
grep -Fqx "issue create" "$triage_log"

rotation_bin="$scratch/rotation-bin"
rotation_capture="$scratch/rotation-bundle"
mkdir "$rotation_bin"
cat >"$rotation_bin/gh" <<'GH'
#!/usr/bin/env bash
set -euo pipefail
test "$1 $2 $3" = "secret set HEYFOOD_CANARY_STATE_BUNDLE_B64"
cat >"${HEYFOOD_CANARY_ROTATION_CAPTURE:?}"
GH
chmod 0700 "$rotation_bin/gh"
PATH="$rotation_bin:$PATH" \
  GH_TOKEN=fixture-short-lived-app-token \
  HEYFOOD_CANARY_ROTATION_CAPTURE="$rotation_capture" \
  "$rotate" "$state" native-eval frntrllc/heyfood
base64 --decode <"$rotation_capture" >"$scratch/rotated-bundle.json"
jq -er '.auth' "$scratch/rotated-bundle.json" |
  base64 --decode >"$scratch/rotated-auth"
jq -er '.credentials' "$scratch/rotated-bundle.json" |
  base64 --decode >"$scratch/rotated-credentials"
cmp "$state/auth.native" "$scratch/rotated-auth"
cmp "$state/credentials.native" "$scratch/rotated-credentials"

echo "production canary contract is fail-closed, non-mutating, and privacy-safe"
