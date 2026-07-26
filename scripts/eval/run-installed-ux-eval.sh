#!/usr/bin/env bash
set -euo pipefail

: "${HEYFOOD_SHOWCASE_ARCHIVE:?HEYFOOD_SHOWCASE_ARCHIVE is required}"
: "${HEYFOOD_SHOWCASE_MANIFEST:?HEYFOOD_SHOWCASE_MANIFEST is required}"
: "${HEYFOOD_SHOWCASE_EVIDENCE_DIR:?HEYFOOD_SHOWCASE_EVIDENCE_DIR is required}"
: "${HEYFOOD_SHOWCASE_TARGET:?HEYFOOD_SHOWCASE_TARGET is required}"
: "${HEYFOOD_SHOWCASE_VERSION:?HEYFOOD_SHOWCASE_VERSION is required}"

rubric=${HEYFOOD_EVAL_RUBRIC:-tests/eval/tui-post-release-rubric.v1.json}
output="$HEYFOOD_SHOWCASE_EVIDENCE_DIR/post-release-eval.json"
mkdir -p "$HEYFOOD_SHOWCASE_EVIDENCE_DIR"

write_failure_report() {
  local category=$1
  local severity=$2
  local observed_status=$3
  local summary=$4
  jq -n \
    --arg version "$HEYFOOD_SHOWCASE_VERSION" \
    --arg target "$HEYFOOD_SHOWCASE_TARGET" \
    --arg category "$category" \
    --arg severity "$severity" \
    --arg observed_status "$observed_status" \
    --arg summary "$summary" \
    '{
      schema_version: 1,
      evaluation: "tui-post-release-v1",
      status: "failed",
      score: 0,
      maximum_score: 100,
      pass_threshold: 100,
      release: {version: $version, target: $target},
      source: {
        installed_artifact: true,
        real_pty: true,
        synthetic_backend: true,
        evidence_file: null
      },
      categories: [{
        id: $category,
        objective: "The public installed-artifact journey harness completes",
        severity: $severity,
        weight: 100,
        status: "failed",
        observed_status: $observed_status
      }],
      findings: [{
        fingerprint: ("tui-post-release-v1:" + $category),
        severity: $severity,
        category: $category,
        summary: $summary
      }],
      limitations: [
        "Inspect the workflow log for the first failed build or terminal assertion."
      ]
    }' >"$output"
}

set +e
cargo test --locked --package heyfood-bin --test installed_showcase --no-run
build_status=$?
set -e
if [[ "$build_status" -ne 0 ]]; then
  write_failure_report \
    "evaluation-infrastructure" \
    "P1" \
    "harness_build_failed" \
    "The installed-artifact evaluator failed to build before a product journey ran"
  exit "$build_status"
fi

set +e
cargo test --locked --package heyfood-bin --test installed_showcase \
  installed_archive_core_release_matrix -- --ignored --exact
harness_status=$?
set -e

if [[ "$harness_status" -ne 0 ]]; then
  write_failure_report \
    "installed-artifact-harness" \
    "P0" \
    "test_process_failed" \
    "The installed-artifact PTY journey failed before complete evidence was produced"
  exit "$harness_status"
fi

cargo xtask evaluate-post-release \
  --evidence-dir "$HEYFOOD_SHOWCASE_EVIDENCE_DIR" \
  --rubric "$rubric" \
  --output "$output"
