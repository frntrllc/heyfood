#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 && $# -ne 5 ]]; then
  echo "usage: triage-production-canary.sh CURRENT PREVIOUS_OR_DASH VERSION RUN_URL [REPOSITORY]" >&2
  exit 64
fi

current=$1
previous=$2
version=$3
run_url=$4
repository=${5:-${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}}

jq -e '
  .schema_version == 1 and
  .evaluation == "heyfood-production-canary-v1" and
  (.status == "passed" or .status == "failed")
' "$current" >/dev/null

status=$(jq -r '.status' "$current")
category=$(jq -r '.failure.category // empty' "$current")
operation=$(jq -r '.failure.operation // empty' "$current")
error_type=$(jq -r '.failure.error_type // empty' "$current")
for value in "$category" "$operation" "$error_type"; do
  [[ -z "$value" || "$value" =~ ^[a-z0-9_]{1,80}$ ]] || {
    echo "canary evidence contains an invalid bounded field" >&2
    exit 65
  }
done

issue_number_for() {
  local fingerprint=$1
  gh issue list \
    --repo "$repository" \
    --state open \
    --search "\"$fingerprint\" in:title" \
    --limit 10 \
    --json number,title \
    --jq "map(select(.title | contains(\"$fingerprint\")))[0].number // empty"
}

close_category() {
  local candidate=$1
  local fingerprint="heyfood-production-canary-v1:$candidate"
  local number
  number=$(issue_number_for "$fingerprint")
  if [[ -n "$number" ]]; then
    gh issue close "$number" \
      --repo "$repository" \
      --comment "Recovered in the production canary for public v$version: $run_url"
  fi
}

if [[ "$status" == "passed" ]]; then
  for candidate in availability contract credential safety evaluation_infrastructure; do
    close_category "$candidate"
  done
  exit 0
fi

if [[ "$category" == "availability" ]]; then
  previous_category=
  if [[ "$previous" != "-" && -f "$previous" ]] &&
    jq -e '
      .schema_version == 1 and
      .evaluation == "heyfood-production-canary-v1"
    ' "$previous" >/dev/null 2>&1; then
    previous_category=$(jq -r '
      select(.status == "failed") |
      .failure.category // empty
    ' "$previous")
  fi
  if [[ "$previous_category" != "availability" ]]; then
    echo "::warning::first production availability failure observed; product incident is deferred until the next consecutive failure"
    exit 0
  fi
fi

case "$category" in
  availability) severity=P1 ;;
  contract | credential | safety) severity=P0 ;;
  evaluation_infrastructure) severity=P1 ;;
  *)
    echo "unknown production canary failure category" >&2
    exit 65
    ;;
esac

fingerprint="heyfood-production-canary-v1:$category"
title="[Production canary][$severity] $category ($fingerprint)"
body=$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/heyfood-canary-issue.XXXXXX")
trap 'rm -f "$body"' EXIT
{
  echo "<!-- heyfood-production-canary -->"
  echo
  echo "The isolated synthetic-account production canary found a reproducible failure."
  echo
  echo "- Fingerprint: \`$fingerprint\`"
  echo "- Public release: \`v$version\`"
  echo "- Operation class: \`$operation\`"
  echo "- Bounded error type: \`$error_type\`"
  echo "- Latest run: $run_url"
  if [[ "$category" == "availability" ]]; then
    echo "- Availability threshold: two consecutive failed canary runs"
  else
    echo "- Deterministic gate: immediate incident"
  fi
  echo
  echo "The evidence intentionally excludes prompts, responses, names, emails, dietary data,"
  echo "tokens, conversation IDs, household IDs, list IDs, and provider data."
  echo
  echo "Resolution requires a product or evaluator correction, a deterministic regression"
  echo "assertion, and a subsequent passing production canary."
} >"$body"

number=$(issue_number_for "$fingerprint")
if [[ -n "$number" ]]; then
  gh issue edit "$number" \
    --repo "$repository" \
    --title "$title" \
    --body-file "$body"
else
  gh issue create \
    --repo "$repository" \
    --label bug \
    --title "$title" \
    --body-file "$body"
fi
