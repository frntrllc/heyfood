#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: rotate-production-canary-state.sh STATE_DIR ENVIRONMENT REPOSITORY" >&2
  exit 64
fi

state_dir=$1
environment=$2
repository=$3
: "${GH_TOKEN:?a short-lived non-personal GitHub App token is required}"

[[ "$environment" == "native-eval" ]] || {
  echo "production canary state may rotate only in native-eval" >&2
  exit 64
}
[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || {
  echo "invalid repository" >&2
  exit 64
}
[[ -d "$state_dir" && ! -L "$state_dir" ]] || {
  echo "credential state directory is unavailable" >&2
  exit 1
}

for required in auth.native credentials.native; do
  test -f "$state_dir/$required"
  test ! -L "$state_dir/$required"
  test -s "$state_dir/$required"
done
if find "$state_dir" -mindepth 1 -maxdepth 1 \
  \( -name '*.reconciliation' -o -name '*authorization-*' \) \
  -print -quit | grep -q .; then
  echo "refusing to rotate unresolved credential state" >&2
  exit 1
fi

bundle=$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/heyfood-canary-bundle.XXXXXX")
cleanup_bundle() {
  if command -v shred >/dev/null 2>&1; then
    shred -u "$bundle"
  else
    : >"$bundle"
    rm -f "$bundle"
  fi
}
trap cleanup_bundle EXIT
jq -n \
  --rawfile auth "$state_dir/auth.native" \
  --rawfile credentials "$state_dir/credentials.native" \
  '{
    schema_version: 1,
    auth: ($auth | @base64),
    credentials: ($credentials | @base64)
  }' |
  base64 |
  tr -d '\n' >"$bundle"
chmod 0600 "$bundle"
gh secret set HEYFOOD_CANARY_STATE_BUNDLE_B64 \
  --env "$environment" \
  --repo "$repository" \
  <"$bundle"
