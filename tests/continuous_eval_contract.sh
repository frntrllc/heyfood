#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

workflow=.github/workflows/continuous-tui-eval.yml
rubric=tests/eval/tui-post-release-rubric.v1.json
runner=scripts/eval/run-installed-ux-eval.sh
triage=scripts/eval/triage-github-issues.sh

test -f "$workflow"
test -f "$rubric"
test -x "$runner"
test -x "$triage"

test "$(jq -r '.schema_version' "$rubric")" = 1
test "$(jq -r '.pass_threshold' "$rubric")" = 100
test "$(jq '[.categories[].weight] | add' "$rubric")" = 100
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
grep -Fq "gh release download" "$workflow"
grep -Fq "gh attestation verify" "$workflow"
grep -Fq "scripts/eval/run-installed-ux-eval.sh" "$workflow"
grep -Fq "issues: write" "$workflow"
grep -Fq "if: always()" "$workflow"
grep -Fq "HEYFOOD_EVAL_EXPECTED_REPORTS: \"4\"" "$workflow"
grep -Fq "tui-post-release-v1:" "$triage"
grep -Fq "gh issue create" "$triage"
grep -Fq "gh issue edit" "$triage"
grep -Fq "gh issue close" "$triage"

if grep -Eq 'api\\.hello\\.food|HEYFOOD_API_URL|grocery[[:space:]]+accept' "$workflow"; then
  echo "continuous evaluation must not silently acquire production mutation authority" >&2
  exit 1
fi

echo "continuous TUI evaluation contract is fail-safe and complete"
