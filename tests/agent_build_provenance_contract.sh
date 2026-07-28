#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT
readonly ASSET="$ROOT/assets/dietary/dietary_options.v2.json"
BACKUP=$(mktemp "${TMPDIR:-/tmp}/heyfood-agent-provenance-asset.XXXXXX")
readonly BACKUP
readonly CHANNEL=candidate

cleanup() {
  cp "$BACKUP" "$ASSET"
  rm -f "$BACKUP"
}
trap cleanup EXIT

cp "$ASSET" "$BACKUP"
cd "$ROOT"

HEYFOOD_DISTRIBUTION_CHANNEL="$CHANNEL" cargo build --locked --package heyfood-bin
before=$(target/debug/heyfood agent describe)
before_digest=$(node -e 'const x=JSON.parse(process.argv[1]); process.stdout.write(x.build.build_input_digest_sha256)' "$before")
node -e 'const x=JSON.parse(process.argv[1]); if (x.build.dirty !== false) process.exit(1)' "$before"

printf '\n' >>"$ASSET"
HEYFOOD_DISTRIBUTION_CHANNEL="$CHANNEL" cargo build --locked --package heyfood-bin
after=$(target/debug/heyfood agent describe)
after_digest=$(node -e 'const x=JSON.parse(process.argv[1]); process.stdout.write(x.build.build_input_digest_sha256)' "$after")
node -e 'const x=JSON.parse(process.argv[1]); if (x.build.dirty !== true) process.exit(1)' "$after"

if [[ "$before_digest" == "$after_digest" ]]; then
  echo "dirty compiled asset did not change the embedded build-input digest" >&2
  exit 1
fi

printf 'agent build provenance contract: dirty asset changed status and digest\n'
