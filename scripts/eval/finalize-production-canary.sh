#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: finalize-production-canary.sh EVIDENCE JOURNEY_OUTCOME ROTATION_OUTCOME CLEANUP_OUTCOME" >&2
  exit 64
fi

evidence=$1
journey=$2
rotation=$3
cleanup=$4

test -f "$evidence"
for outcome in "$journey" "$rotation" "$cleanup"; do
  case "$outcome" in
    success | failure | skipped | cancelled) ;;
    *)
      echo "invalid production canary step outcome" >&2
      exit 64
      ;;
  esac
done

jq -e '
  .schema_version == 1 and
  .evaluation == "heyfood-production-canary-v1" and
  (.status == "passed" or .status == "failed")
' "$evidence" >/dev/null

category=
operation=
error_type=
if [[ "$rotation" != "success" ]]; then
  category=credential
  operation=state_rotation
  error_type=protected_state_rotation_failed
elif [[ "$cleanup" != "success" ]]; then
  category=credential
  operation=local_cleanup
  error_type=credential_material_cleanup_failed
elif [[ "$journey" != "success" ]] &&
  [[ "$(jq -r '.status' "$evidence")" == "passed" ]]; then
  category=evaluation_infrastructure
  operation=journey_enforcement
  error_type=journey_step_failed_after_pass_report
fi

staged=$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/heyfood-canary-finalize.XXXXXX")
trap 'rm -f "$staged"' EXIT
jq \
  --arg journey "$journey" \
  --arg rotation "$rotation" \
  --arg cleanup "$cleanup" \
  --arg category "$category" \
  --arg operation "$operation" \
  --arg error_type "$error_type" \
  '
    .postconditions = {
      journey: $journey,
      protected_state_rotation: $rotation,
      credential_material_cleanup: $cleanup
    } |
    if $category == "" then .
    else
      .status = "failed" |
      .failure = {
        category: $category,
        operation: $operation,
        error_type: $error_type
      }
    end
  ' "$evidence" >"$staged"
chmod 0600 "$staged"
mv "$staged" "$evidence"
trap - EXIT
