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

set +e
cargo test --locked --package heyfood-bin --test installed_showcase \
  installed_archive_core_release_matrix -- --ignored --exact
harness_status=$?
set -e

if [[ "$harness_status" -ne 0 ]]; then
  jq -n \
    --arg version "$HEYFOOD_SHOWCASE_VERSION" \
    --arg target "$HEYFOOD_SHOWCASE_TARGET" \
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
        id: "installed-artifact-harness",
        objective: "The public installed-artifact journey harness completes",
        severity: "P0",
        weight: 100,
        status: "failed",
        observed_status: "test_process_failed"
      }],
      findings: [{
        fingerprint: "tui-post-release-v1:installed-artifact-harness",
        severity: "P0",
        category: "installed-artifact-harness",
        summary: "The installed-artifact PTY harness failed before evidence was complete"
      }],
      limitations: [
        "Inspect the workflow log for the first failed terminal assertion."
      ]
    }' >"$output"
  exit "$harness_status"
fi

cargo xtask evaluate-post-release \
  --evidence-dir "$HEYFOOD_SHOWCASE_EVIDENCE_DIR" \
  --rubric "$rubric" \
  --output "$output"
