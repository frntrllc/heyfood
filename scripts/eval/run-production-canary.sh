#!/usr/bin/env bash
set -euo pipefail

: "${HEYFOOD_CANARY_BINARY:?HEYFOOD_CANARY_BINARY is required}"
: "${HEYFOOD_CANARY_STATE_DIR:?HEYFOOD_CANARY_STATE_DIR is required}"
: "${HEYFOOD_CANARY_EVIDENCE_DIR:?HEYFOOD_CANARY_EVIDENCE_DIR is required}"
: "${HEYFOOD_CANARY_VERSION:?HEYFOOD_CANARY_VERSION is required}"
: "${HEYFOOD_CANARY_TARGET:?HEYFOOD_CANARY_TARGET is required}"
: "${HEYFOOD_CANARY_ARCHIVE_SHA256:?HEYFOOD_CANARY_ARCHIVE_SHA256 is required}"

case "$HEYFOOD_CANARY_VERSION" in
  *[!0-9.]* | *.*.*.* | .* | *.) echo "invalid canary version" >&2; exit 64 ;;
esac
[[ "$HEYFOOD_CANARY_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "invalid canary version" >&2
  exit 64
}
[[ "$HEYFOOD_CANARY_TARGET" == "x86_64-unknown-linux-gnu" ]] || {
  echo "the production canary supports only the qualified Linux x86-64 archive" >&2
  exit 64
}
[[ "$HEYFOOD_CANARY_ARCHIVE_SHA256" =~ ^[0-9a-f]{64}$ ]] || {
  echo "invalid archive digest" >&2
  exit 64
}
[[ "$HEYFOOD_CANARY_BINARY" = /* && -x "$HEYFOOD_CANARY_BINARY" ]] || {
  echo "canary binary must be an absolute executable path" >&2
  exit 64
}
[[ "$HEYFOOD_CANARY_STATE_DIR" = /* && -d "$HEYFOOD_CANARY_STATE_DIR" ]] || {
  echo "canary state directory must be an absolute directory" >&2
  exit 64
}
[[ "$HEYFOOD_CANARY_EVIDENCE_DIR" = /* ]] || {
  echo "canary evidence directory must be absolute" >&2
  exit 64
}

umask 077
mkdir -p "$HEYFOOD_CANARY_EVIDENCE_DIR"
output="$HEYFOOD_CANARY_EVIDENCE_DIR/production-canary.json"
scratch=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/heyfood-production-canary.XXXXXX")
operations="$scratch/operations.ndjson"
touch "$operations"

cleanup() {
  find "$scratch" -type f -exec sh -c '
    for path do
      if command -v shred >/dev/null 2>&1; then
        shred -u "$path"
      else
        : >"$path"
        rm -f "$path"
      fi
    done
  ' sh {} +
  rmdir "$scratch" 2>/dev/null || true
}
trap cleanup EXIT

write_report() {
  local status=$1
  local category=${2:-}
  local operation=${3:-}
  local error_type=${4:-}
  local non_mutation=${5:-false}
  local operations_json
  operations_json=$(jq -s '.' "$operations")
  jq -n \
    --arg status "$status" \
    --arg version "$HEYFOOD_CANARY_VERSION" \
    --arg target "$HEYFOOD_CANARY_TARGET" \
    --arg digest "$HEYFOOD_CANARY_ARCHIVE_SHA256" \
    --arg category "$category" \
    --arg operation "$operation" \
    --arg error_type "$error_type" \
    --argjson non_mutation "$non_mutation" \
    --argjson operations "$operations_json" \
    '{
      schema_version: 1,
      evaluation: "heyfood-production-canary-v1",
      status: $status,
      release: {
        version: $version,
        target: $target,
        archive_sha256: $digest
      },
      provenance: {
        public_installed_artifact: true,
        production_api: true,
        synthetic_account: true,
        provider_tokens_present: false
      },
      mutation_policy: {
        proposal_prepared: false,
        decision: null,
        non_mutation_verified: $non_mutation,
        accept_permitted: false
      },
      operations: $operations,
      failure: (
        if $status == "passed" then null
        else {
          category: $category,
          operation: $operation,
          error_type: $error_type
        }
        end
      ),
      privacy: {
        raw_requests_retained: false,
        raw_responses_retained: false,
        identifiers_retained: false,
        credentials_retained: false
      }
    }' >"$output"
}

write_report "failed" "evaluation_infrastructure" "initialize" "workflow_incomplete"

file_mode() {
  if stat -c '%a' "$1" >/dev/null 2>&1; then
    stat -c '%a' "$1"
  else
    stat -f '%Lp' "$1"
  fi
}

now_nanoseconds() {
  local value
  value=$(date +%s%N)
  if [[ "$value" =~ ^[0-9]{19}$ ]]; then
    printf '%s' "$value"
  else
    printf '%s000000000' "$(date +%s)"
  fi
}

for required in auth.native credentials.native; do
  path="$HEYFOOD_CANARY_STATE_DIR/$required"
  [[ -f "$path" && ! -L "$path" && -s "$path" ]] || {
    write_report "failed" "credential" "state_preflight" "credential_state_missing"
    exit 1
  }
  mode=$(file_mode "$path")
  (( 8#$mode <= 8#600 )) || {
    write_report "failed" "credential" "state_preflight" "credential_state_permissions"
    exit 1
  }
done

state_mode=$(file_mode "$HEYFOOD_CANARY_STATE_DIR")
(( 8#$state_mode <= 8#700 )) || {
  write_report "failed" "credential" "state_preflight" "credential_directory_permissions"
  exit 1
}
if find "$HEYFOOD_CANARY_STATE_DIR" -mindepth 1 -maxdepth 1 -type l -print -quit |
  grep -q .; then
  write_report "failed" "credential" "state_preflight" "credential_state_symlink"
  exit 1
fi

export HEYFOOD_STATE_DIR="$HEYFOOD_CANARY_STATE_DIR"
export HEYFOOD_CREDENTIAL_STORE=file
export HEYFOOD_API_URL=https://api.hello.food
unset HEYFOOD_API_KEY

classify_error() {
  case "$1" in
    request_transport | response_transport | sse_inactivity | service_unavailable | rate_limited | \
      http_status)
      printf 'availability'
      ;;
    login_required | scope_required | authorization_scope_upgrade_required | \
      channel_refresh_* | session_* | credential_* | reauthorization_*)
      printf 'credential'
      ;;
    *)
      printf 'contract'
      ;;
  esac
}

record_operation() {
  local id=$1
  local status=$2
  local latency_ms=$3
  jq -cn \
    --arg id "$id" \
    --arg status "$status" \
    --argjson latency_ms "$latency_ms" \
    '{id: $id, status: $status, latency_ms: $latency_ms}' >>"$operations"
}

run_operation() {
  local id=$1
  local stdout_path=$2
  local stdin_path=${3:-}
  shift 3
  local started_ns ended_ns latency_ms error_type category
  started_ns=$(now_nanoseconds)
  if [[ -n "$stdin_path" ]]; then
    if timeout --signal=TERM --kill-after=5s 90s \
      "$HEYFOOD_CANARY_BINARY" "$@" \
      <"$stdin_path" >"$stdout_path" 2>"$scratch/$id.stderr"; then
      :
    else
      ended_ns=$(now_nanoseconds)
      latency_ms=$(( (ended_ns - started_ns) / 1000000 ))
      error_type=$(jq -r '.error.type // "process_failure"' "$stdout_path" 2>/dev/null ||
        printf 'process_failure')
      [[ "$error_type" =~ ^[a-z0-9_]{1,80}$ ]] || error_type=invalid_error_envelope
      category=$(classify_error "$error_type")
      record_operation "$id" "failed" "$latency_ms"
      write_report "failed" "$category" "$id" "$error_type"
      exit 1
    fi
  elif timeout --signal=TERM --kill-after=5s 90s \
    "$HEYFOOD_CANARY_BINARY" "$@" \
    >"$stdout_path" 2>"$scratch/$id.stderr"; then
    :
  else
    ended_ns=$(now_nanoseconds)
    latency_ms=$(( (ended_ns - started_ns) / 1000000 ))
    error_type=$(jq -r '.error.type // "process_failure"' "$stdout_path" 2>/dev/null ||
      printf 'process_failure')
    [[ "$error_type" =~ ^[a-z0-9_]{1,80}$ ]] || error_type=invalid_error_envelope
    category=$(classify_error "$error_type")
    record_operation "$id" "failed" "$latency_ms"
    write_report "failed" "$category" "$id" "$error_type"
    exit 1
  fi
  ended_ns=$(now_nanoseconds)
  latency_ms=$(( (ended_ns - started_ns) / 1000000 ))
  record_operation "$id" "passed" "$latency_ms"
}

common=(--json --no-color --no-banner --no-input)
before="$scratch/grocery-before.json"
run_operation grocery_read "$before" "" "${common[@]}" grocery list
if ! jq -e '
  (.id | type == "string" and test(
    "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$"
  )) and
  (.version | type == "number" and . >= 1 and floor == .) and
  (.items | type == "array")
' "$before" >/dev/null; then
  write_report "failed" "contract" "grocery_read" "grocery_list_contract"
  exit 1
fi
list_id=$(jq -er '.id' "$before")
list_version=$(jq -er '.version' "$before")
before_digest=$(jq -S -c '.' "$before" | sha256sum | cut -d' ' -f1)

prompt="$scratch/prompt"
printf '%s\n' \
  'For this automated synthetic canary, reply with one short availability acknowledgement.' \
  >"$prompt"
agent="$scratch/agent.json"
run_operation agent_turn "$agent" "$prompt" "${common[@]}" ask
if ! jq -e '
  type == "object" and
  (.error? == null) and
  ((.message? | type == "string") or
   (.text? | type == "string") or
   (.response? | type == "string"))
' "$agent" >/dev/null; then
  write_report "failed" "contract" "agent_turn" "agent_result_contract"
  exit 1
fi

after="$scratch/grocery-after.json"
run_operation grocery_nonmutation "$after" "" "${common[@]}" grocery list
if ! jq -e \
  --arg id "$list_id" \
  --argjson version "$list_version" \
  '.id == $id and .version == $version and (.items | type == "array")' \
  "$after" >/dev/null; then
  write_report "failed" "safety" "grocery_nonmutation" "list_authority_changed"
  exit 1
fi
after_digest=$(jq -S -c '.' "$after" | sha256sum | cut -d' ' -f1)
if [[ "$before_digest" != "$after_digest" ]]; then
  write_report "failed" "safety" "grocery_nonmutation" "cancel_mutated_list"
  exit 1
fi

if find "$HEYFOOD_CANARY_STATE_DIR" -mindepth 1 -maxdepth 1 \
  \( -name '*.reconciliation' -o -name '*authorization-*' \) \
  -print -quit | grep -q .; then
  write_report "failed" "credential" "state_reconciliation" "credential_state_uncertain" "true"
  exit 1
fi

write_report "passed" "" "" "" "true"
