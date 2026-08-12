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

issues=$(gh issue list \
  --repo "$repository" \
  --state open \
  --search "\"heyfood-production-canary-v1:v$version:\" in:title" \
  --limit 100 \
  --json title,body,author,labels)
open_incidents=$(jq -r --arg version "$version" '
  map(select(
    .author.is_bot == true and
    .author.login == "app/github-actions" and
    any(.labels[]?; .name == "bug") and
    (.body | startswith("<!-- heyfood-production-canary -->")) and
    (.title | capture(
      "^\\[Production canary\\]\\[P[01]\\] (?<category>availability|contract|credential|safety|evaluation_infrastructure) \\(heyfood-production-canary-v1:v(?<version>[0-9]+\\.[0-9]+\\.[0-9]+):(?<target>[a-z0-9_][a-z0-9_.-]{1,79}):(?<digest>[0-9a-f]{64}|unavailable):(?<fingerprint_category>availability|contract|credential|safety|evaluation_infrastructure)\\)$"
    )) as $match |
    $match.version == $version and
    $match.category == $match.fingerprint_category
  )) | length
' <<<"$issues")
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
