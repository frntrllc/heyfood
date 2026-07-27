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
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "invalid public version" >&2
  exit 64
}

jq -e --arg version "$version" '
  .schema_version == 1 and
  .evaluation == "heyfood-production-canary-v1" and
  .release.version == $version and
  (.status == "passed" or .status == "failed")
' "$current" >/dev/null

status=$(jq -r '.status' "$current")
category=$(jq -r '.failure.category // empty' "$current")
operation=$(jq -r '.failure.operation // empty' "$current")
error_type=$(jq -r '.failure.error_type // empty' "$current")
target=$(jq -r '.release.target' "$current")
archive_digest=$(jq -r '.release.archive_sha256' "$current")
job_result=${HEYFOOD_CANARY_JOB_RESULT:-success}
case "$job_result" in
  success | failure | skipped | cancelled) ;;
  *)
    echo "invalid production canary job result" >&2
    exit 65
    ;;
esac
if [[ "$status" == "passed" && "$job_result" != "success" ]]; then
  status=failed
  category=evaluation_infrastructure
  operation=workflow_enforcement
  error_type=job_failed_after_pass_report
fi
for value in "$category" "$operation" "$error_type"; do
  [[ -z "$value" || "$value" =~ ^[a-z0-9_]{1,80}$ ]] || {
    echo "canary evidence contains an invalid bounded field" >&2
    exit 65
  }
done
[[ "$target" =~ ^[a-z0-9_][a-z0-9_.-]{1,79}$ ]] || {
  echo "canary evidence contains an invalid target" >&2
  exit 65
}
if [[ ! "$archive_digest" =~ ^[0-9a-f]{64}$ ]] &&
  [[ ! "$archive_digest" == "unavailable" || "$status" != "failed" ]]; then
  echo "canary evidence contains an invalid archive digest" >&2
  exit 65
fi

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

close_category_for_digest() {
  local candidate=$1
  local digest=$2
  local fingerprint="heyfood-production-canary-v1:v$version:$target:$digest:$candidate"
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
    close_category_for_digest "$candidate" "$archive_digest"
  done
  close_category_for_digest credential unavailable
  close_category_for_digest evaluation_infrastructure unavailable
  exit 0
fi

if [[ "$category" == "availability" ]]; then
  previous_category=
  if [[ "$previous" != "-" && -f "$previous" ]] &&
    jq -e \
      --arg version "$version" \
      --arg target "$target" \
      --arg digest "$archive_digest" '
      .schema_version == 1 and
      .evaluation == "heyfood-production-canary-v1" and
      .release.version == $version and
      .release.target == $target and
      .release.archive_sha256 == $digest
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

fingerprint="heyfood-production-canary-v1:v$version:$target:$archive_digest:$category"
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
