#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

workflow=.github/workflows/continuous-tui-eval.yml
rubric=tests/eval/tui-post-release-rubric.v2.json
runner=scripts/eval/run-installed-ux-eval.sh
triage=scripts/eval/triage-github-issues.sh

test -f "$workflow"
test -f "$rubric"
test -x "$runner"
test -x "$triage"

test "$(jq -r '.schema_version' "$rubric")" = 1
test "$(jq -r '.pass_threshold' "$rubric")" = 100
test "$(jq '[.categories[].weight] | add' "$rubric")" = 100
test "$(jq -r '.id' "$rubric")" = "tui-post-release-v2"
test "$(jq -r '.categories[] | select(.id == "first-run-orientation") | .severity' "$rubric")" = "P2"
test "$(
  jq -r '.categories[].id' "$rubric" | LC_ALL=C sort -u | wc -l | tr -d ' '
)" = "$(
  jq '.categories | length' "$rubric"
)"

for target in \
  aarch64-apple-darwin \
  x86_64-apple-darwin \
  aarch64-unknown-linux-gnu \
  x86_64-unknown-linux-gnu
do
  grep -Fq "target: $target" "$workflow"
done

grep -Fq 'cron: "17 9 * * *"' "$workflow"
grep -Fq "sudo apt-get install --yes libdbus-1-dev pkg-config" "$workflow"
grep -Fq "gh release download" "$workflow"
grep -Fq "gh attestation verify" "$workflow"
grep -Fq "scripts/eval/run-installed-ux-eval.sh" "$workflow"
grep -Fq "issues: write" "$workflow"
grep -Fq "if: always()" "$workflow"
grep -Fq "HEYFOOD_EVAL_EXPECTED_REPORTS: \"4\"" "$workflow"
# shellcheck disable=SC2016 # These are literal implementation-contract fragments.
grep -Fq 'rubric_id=$(jq -er' "$triage"
# shellcheck disable=SC2016 # These are literal implementation-contract fragments.
grep -Fq 'fingerprint="$rubric_id:$category"' "$triage"
grep -Fq "gh issue create" "$triage"
grep -Fq "gh issue edit" "$triage"
grep -Fq "gh issue close" "$triage"
grep -Fq -- "--no-run" "$runner"
grep -Fq '"evaluation-infrastructure"' "$runner"
grep -Fq '"installed-artifact-harness"' "$runner"

if grep -Eq 'api\\.hello\\.food|HEYFOOD_API_URL|grocery[[:space:]]+accept' "$workflow"; then
  echo "continuous evaluation must not silently acquire production mutation authority" >&2
  exit 1
fi

echo "continuous TUI evaluation contract is fail-safe and complete"
