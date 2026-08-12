#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: check-production-canary-recurrence.sh EVENT_NAME SCHEDULE_ENABLED VERSION REPOSITORY" >&2
  exit 64
fi

event_name=$1
schedule_enabled=$2
version=$3
repository=$4
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "invalid public version" >&2
  exit 64
}
[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || {
  echo "invalid repository" >&2
  exit 64
}

if [[ "$event_name" == "workflow_dispatch" ]]; then
  printf '%s\n' true
  exit 0
fi
if [[ "$event_name" != "schedule" ]]; then
  echo "unsupported production canary event" >&2
  exit 64
fi
if [[ "$schedule_enabled" != "true" ]]; then
  echo "::notice::scheduled production canary is administratively disabled" >&2
  printf '%s\n' false
  exit 0
fi

open_incidents=$(gh issue list \
  --repo "$repository" \
  --state open \
  --search "\"heyfood-production-canary-v1:v$version:\" in:title" \
  --limit 100 \
  --json title \
  --jq "map(select(.title | startswith(\"[Production canary]\") and contains(\"heyfood-production-canary-v1:v$version:\"))) | length")
[[ "$open_incidents" =~ ^[0-9]+$ ]] || {
  echo "invalid open-incident count" >&2
  exit 65
}
if ((open_incidents > 0)); then
  echo "::notice::scheduled production canary is suspended by an open production-canary incident" >&2
  printf '%s\n' false
else
  printf '%s\n' true
fi
