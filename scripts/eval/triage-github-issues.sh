#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: triage-github-issues.sh REPORT_ROOT RUBRIC VERSION RUN_URL" >&2
  exit 64
fi

report_root=$1
rubric=$2
version=$3
run_url=$4
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
rubric_id=$(jq -er '.id' "$rubric")

scratch=$(mktemp -d "${TMPDIR:-/tmp}/heyfood-eval-triage.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
reports_file="$scratch/reports.txt"
find "$report_root" -type f -name post-release-eval.json -print | LC_ALL=C sort >"$reports_file"

report_count=$(wc -l <"$reports_file" | tr -d ' ')
expected_reports=${HEYFOOD_EVAL_EXPECTED_REPORTS:-4}
failed_categories="$scratch/failed-categories.txt"
if [[ "$report_count" -gt 0 ]]; then
  xargs jq -r '.categories[]? | select(.status != "passed") | .id' \
    <"$reports_file" | LC_ALL=C sort -u >"$failed_categories"
else
  : >"$failed_categories"
fi
if [[ "$report_count" -ne "$expected_reports" ]]; then
  echo "evaluation-infrastructure" >>"$failed_categories"
  LC_ALL=C sort -u -o "$failed_categories" "$failed_categories"
fi

all_categories="$scratch/all-categories.txt"
jq -r '.categories[].id' "$rubric" >"$all_categories"
printf '%s\n' installed-artifact-harness evaluation-infrastructure >>"$all_categories"
LC_ALL=C sort -u -o "$all_categories" "$all_categories"

issue_number_for() {
  local fingerprint=$1
  gh issue list \
    --repo "$GITHUB_REPOSITORY" \
    --state open \
    --search "\"$fingerprint\" in:title" \
    --limit 10 \
    --json number,title \
    --jq "map(select(.title | contains(\"$fingerprint\")))[0].number // empty"
}

issue_body_for() {
  local category=$1
  local fingerprint=$2
  local body=$3
  {
    echo "<!-- heyfood-automated-eval -->"
    echo
    echo "The continuous installed-artifact UX evaluation found a reproducible regression."
    echo
    echo "- Fingerprint: \`$fingerprint\`"
    echo "- Category: \`$category\`"
    echo "- Public release: \`v$version\`"
    echo "- Latest run: $run_url"
    echo "- Reports observed: \`$report_count/$expected_reports\`"
    echo
    echo "Affected targets:"
    if [[ "$report_count" -gt 0 ]]; then
      while IFS= read -r report; do
        jq -r --arg category "$category" '
          select(any(.categories[]?; .id == $category and .status != "passed")) |
          "- `" + (.release.target // "unknown-target") + "` — score `" +
          ((.score // 0) | tostring) + "/100`"
        ' "$report"
      done <"$reports_file"
    fi
    if [[ "$category" == "evaluation-infrastructure" ]]; then
      echo "- one or more target reports were not uploaded"
    fi
    echo
    echo "Resolution contract:"
    echo
    echo "1. Reproduce against the same public archive."
    echo "2. Fix the product or the evaluator if the contract is wrong."
    echo "3. Add or strengthen the deterministic assertion."
    echo "4. Merge only after the affected installed-artifact matrix is green."
    echo "5. The next complete scheduled run closes this issue automatically."
  } >"$body"
}

while IFS= read -r category; do
  [[ -n "$category" ]] || continue
  if grep -Fqx "$category" "$failed_categories"; then
    fingerprint="$rubric_id:$category"
    severity=$(
      jq -r --arg category "$category" \
        '.categories[] | select(.id == $category) | .severity' "$rubric"
    )
    if [[ -z "$severity" && "$report_count" -gt 0 ]]; then
      severity=$(
        # shellcheck disable=SC2016 # `$category` is a jq variable.
        xargs jq -s -r --arg category "$category" \
          '[.[] | .categories[]? | select(.id == $category) | .severity // empty][0] // empty' \
          <"$reports_file"
      )
    fi
    severity=${severity:-P1}
    title="[Automated UX eval][$severity] $category ($fingerprint)"
    body="$scratch/$category.md"
    issue_body_for "$category" "$fingerprint" "$body"
    number=$(issue_number_for "$fingerprint")
    if [[ -n "$number" ]]; then
      gh issue edit "$number" \
        --repo "$GITHUB_REPOSITORY" \
        --title "$title" \
        --body-file "$body"
    else
      gh issue create \
        --repo "$GITHUB_REPOSITORY" \
        --label bug \
        --title "$title" \
        --body-file "$body"
    fi
  elif [[ "$report_count" -eq "$expected_reports" ]]; then
    fingerprint="$rubric_id:$category"
    number=$(issue_number_for "$fingerprint")
    if [[ -n "$number" ]]; then
      gh issue close "$number" \
        --repo "$GITHUB_REPOSITORY" \
        --comment "Recovered in the complete v$version installed-artifact matrix: $run_url"
    fi
  fi
done <"$all_categories"
