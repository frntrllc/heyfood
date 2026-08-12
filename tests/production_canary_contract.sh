#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

workflow=.github/workflows/production-tui-canary.yml
runner=scripts/eval/run-production-canary.sh
triage=scripts/eval/triage-production-canary.sh
finalize=scripts/eval/finalize-production-canary.sh
rotate=scripts/eval/rotate-production-canary-state.sh
recurrence=scripts/eval/check-production-canary-recurrence.sh

test -f "$workflow"
test -x "$runner"
test -x "$triage"
test -x "$finalize"
test -x "$rotate"
test -x "$recurrence"

grep -Fq 'cron: "23 */6 * * *"' "$workflow"
grep -Fq "environment: native-eval" "$workflow"
grep -Fq "github.ref == 'refs/heads/main'" "$workflow"
grep -Fq "scripts/eval/check-production-canary-recurrence.sh" "$workflow"
grep -Fq "SCHEDULE_ENABLED: \${{ vars.HEYFOOD_PRODUCTION_CANARY_ENABLED }}" "$workflow"
grep -Fq "issues: read" "$workflow"
grep -Fq "HEYFOOD_CANARY_STATE_BUNDLE_B64" "$workflow"
grep -Fq "HEYFOOD_CANARY_ROTATOR_APP_ID" "$workflow"
grep -Fq "HEYFOOD_CANARY_ROTATOR_APP_PRIVATE_KEY" "$workflow"
grep -Fq "permission-environments: write" "$workflow"
if grep -Fq "actions: write" "$workflow"; then
  echo "production canary workflow must not request ineffective repository-variable write authority" >&2
  exit 1
fi
grep -Fq "HEYFOOD_CANARY_INCIDENT_ASSIGNEE: \${{ vars.HEYFOOD_PRODUCTION_CANARY_ASSIGNEE }}" "$workflow"
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

recurrence_bin="$scratch/recurrence-bin"
mkdir "$recurrence_bin"
cat >"$recurrence_bin/gh" <<'GH'
#!/usr/bin/env bash
set -euo pipefail
test "$1 $2" = "issue list"
case "${HEYFOOD_CANARY_ISSUE_FIXTURE:-none}" in
  none) printf '%s\n' '[]' ;;
  authentic)
    printf '%s\n' '[{"author":{"is_bot":true,"login":"app/github-actions"},"labels":[{"name":"bug"}],"body":"<!-- heyfood-production-canary -->\nincident","title":"[Production canary][P0] credential (heyfood-production-canary-v1:v0.9.0:x86_64-unknown-linux-gnu:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:credential)"}]'
    ;;
  spoof)
    printf '%s\n' '[{"author":{"is_bot":false,"login":"external-user"},"labels":[{"name":"bug"}],"body":"<!-- heyfood-production-canary -->\nincident","title":"[Production canary][P0] credential (heyfood-production-canary-v1:v0.9.0:x86_64-unknown-linux-gnu:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:credential)"}]'
    ;;
  malformed)
    printf '%s\n' '[{"author":{"is_bot":true,"login":"app/github-actions"},"labels":[{"name":"bug"}],"body":"<!-- heyfood-production-canary -->\nincident","title":"[Production canary] heyfood-production-canary-v1:v0.9.0:"}]'
    ;;
  api_error) exit 1 ;;
  *) exit 64 ;;
esac
GH
chmod 0700 "$recurrence_bin/gh"
test "$(PATH="$recurrence_bin:$PATH" "$recurrence" workflow_dispatch false 0.9.0 frntrllc/heyfood)" = true
test "$(PATH="$recurrence_bin:$PATH" "$recurrence" schedule false 0.9.0 frntrllc/heyfood)" = false
test "$(PATH="$recurrence_bin:$PATH" HEYFOOD_CANARY_ISSUE_FIXTURE=authentic \
  "$recurrence" schedule true 0.9.0 frntrllc/heyfood)" = false
test "$(PATH="$recurrence_bin:$PATH" HEYFOOD_CANARY_ISSUE_FIXTURE=none \
  "$recurrence" schedule true 0.9.0 frntrllc/heyfood)" = true
test "$(PATH="$recurrence_bin:$PATH" HEYFOOD_CANARY_ISSUE_FIXTURE=spoof \
  "$recurrence" schedule true 0.9.0 frntrllc/heyfood)" = true
test "$(PATH="$recurrence_bin:$PATH" HEYFOOD_CANARY_ISSUE_FIXTURE=malformed \
  "$recurrence" schedule true 0.9.0 frntrllc/heyfood)" = true
if PATH="$recurrence_bin:$PATH" HEYFOOD_CANARY_ISSUE_FIXTURE=api_error \
  "$recurrence" schedule true 0.9.0 frntrllc/heyfood; then
  echo "production recurrence gate failed open on an issue API error" >&2
  exit 1
fi
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
  printf '%s' "${HEYFOOD_CANARY_EXISTING_ISSUE:-}"
  exit 0
fi
if [[ "$1 $2" == "issue create" ]]; then
  printf '%s\n' "$1 $2" >>"${HEYFOOD_CANARY_TRIAGE_LOG:?}"
  printf '%s\n' "https://example.invalid/issues/123"
  exit 0
fi
printf '%s\n' "$1 $2" >>"${HEYFOOD_CANARY_TRIAGE_LOG:?}"
if [[ "$1 $2" == "issue edit" && " $* " == *" --add-assignee "* &&
  "${HEYFOOD_CANARY_FORCE_ASSIGNMENT_FAILURE:-0}" == 1 ]]; then
  exit 1
fi
GH
chmod 0700 "$triage_bin/gh"

PATH="$triage_bin:$PATH" \
  HEYFOOD_CANARY_TRIAGE_LOG="$triage_log" \
  HEYFOOD_CANARY_INCIDENT_ASSIGNEE=release-owner \
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
  HEYFOOD_CANARY_INCIDENT_ASSIGNEE=release-owner \
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
  HEYFOOD_CANARY_INCIDENT_ASSIGNEE=release-owner \
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
  HEYFOOD_CANARY_INCIDENT_ASSIGNEE=release-owner \
  "$triage" \
  "$evidence-mutation/production-canary.json" \
  - \
  0.5.0 \
  https://example.invalid/safety \
  frntrllc/heyfood
grep -Fqx "issue create" "$triage_log"
grep -Fqx "issue edit" "$triage_log"

: >"$triage_log"
if PATH="$triage_bin:$PATH" \
  HEYFOOD_CANARY_TRIAGE_LOG="$triage_log" \
  "$triage" \
  "$evidence-mutation/production-canary.json" \
  - \
  0.5.0 \
  https://example.invalid/missing-owner \
  frntrllc/heyfood; then
  echo "deterministic triage accepted a missing incident owner" >&2
  exit 1
fi
grep -Fqx "issue create" "$triage_log"

: >"$triage_log"
if PATH="$triage_bin:$PATH" \
  HEYFOOD_CANARY_TRIAGE_LOG="$triage_log" \
  HEYFOOD_CANARY_INCIDENT_ASSIGNEE=unassignable-owner \
  HEYFOOD_CANARY_FORCE_ASSIGNMENT_FAILURE=1 \
  "$triage" \
  "$evidence-mutation/production-canary.json" \
  - \
  0.5.0 \
  https://example.invalid/unassignable-owner \
  frntrllc/heyfood; then
  echo "deterministic triage swallowed an incident assignment failure" >&2
  exit 1
fi
grep -Fqx "issue create" "$triage_log"
grep -Fqx "issue edit" "$triage_log"
test "$(tail -n 1 "$triage_log")" = "issue edit"

: >"$triage_log"
PATH="$triage_bin:$PATH" \
  HEYFOOD_CANARY_TRIAGE_LOG="$triage_log" \
  HEYFOOD_CANARY_EXISTING_ISSUE=123 \
  HEYFOOD_CANARY_INCIDENT_ASSIGNEE=release-owner \
  "$triage" \
  "$evidence-mutation/production-canary.json" \
  - \
  0.5.0 \
  https://example.invalid/safety-repeat \
  frntrllc/heyfood
grep -Fqx "issue edit" "$triage_log"

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
  HEYFOOD_CANARY_INCIDENT_ASSIGNEE=release-owner \
  "$triage" \
  "$postcondition_failure" \
  - \
  0.5.0 \
  https://example.invalid/rotation \
  frntrllc/heyfood
grep -Fqx "issue create" "$triage_log"
grep -Fqx "issue edit" "$triage_log"

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
