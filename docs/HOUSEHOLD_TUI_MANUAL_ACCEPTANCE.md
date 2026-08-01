# Native household TUI manual acceptance

This is the required human-attached-terminal acceptance pass for the v0.6.3
native household lifecycle and managed-install boundary. Do not automate the
TUI through a PTY, capture the terminal, take screenshots, or record household
labels, stable IDs, profile answers, account identifiers, paths, credentials,
tokens, vault bytes, or authorization responses.

## Exact candidate retrieval and installer transport

Run protected `Native CLI CI` with `qualify_signed_candidate=true` at the exact
proposed `main` commit. After all protected jobs pass, download only that run's
aggregate artifact and calculate the digest that closes the release set:

```bash
approved_run_id=GITHUB_ACTIONS_RUN_ID
candidate_directory=/absolute/path/to/empty/candidate-release
gh run download "$approved_run_id" \
  --repo frntrllc/heyfood \
  --name protected-candidate-release-set \
  --dir "$candidate_directory"
approved_manifest_sha256=$(
  shasum -a 256 "$candidate_directory/SHA256SUMS" | awk '{print $1}'
)
```

Verify every downloaded asset's protected provenance before using it:

```bash
for asset in \
  "$candidate_directory"/*.tar.gz \
  "$candidate_directory"/*.json \
  "$candidate_directory"/SHA256SUMS; do
  gh attestation verify "$asset" --repo frntrllc/heyfood
done
```

From the clean checkout at that same commit, use the checked-in content-free
transport fixture whenever a journey invokes the reviewed installer:

```bash
scripts/release/candidate-transport.sh \
  "$candidate_directory" \
  0.6.3 \
  "$approved_manifest_sha256" \
  ./install.sh
```

The fixture verifies the exact ten-file set and approved manifest digest, then
serves only checksum-bound candidate assets at the installer's expected HTTPS
release URLs. It does not change `install.sh`, contact a release endpoint,
launch the TUI, automate a terminal, or record asset contents. `HOME`,
`HEYFOOD_BIN_DIR`, and `HEYFOOD_STATE_DIR` still select the isolated profile
for the journey.

## Preconditions

- Use isolated local OS profiles and disposable hello.food test accounts.
- Use the exact protected v0.6.3 candidate product/verifier pair, declaration,
  and `SHA256SUMS` intended for publication. Record only the candidate version
  and approved digest.
- Before publication, route installer downloads through the checked-in
  `scripts/release/candidate-transport.sh` fixture using the exact invocation
  above. It substitutes transport only and must not be used to automate the
  TUI.
- Retain the immutable public v0.6.2 installer and host product archive only to
  prepare the pre-migration side of the upgrade journey; do not alter that
  release and do not execute its installer or binary after native migration.
- On macOS, use the signed and notarized candidate. On Linux, use the exact
  attested candidate. Confirm the target's public-set attestation before any
  TUI journey.
- Enable the reviewed native-household rollout for the candidate.
- Arrange one eligible existing-member fixture for `/onboard --for`; the
  fixture must contain no production identity or dietary data.
- Arrange a content-free test control that can expire/rotate the disposable
  account's application session without disclosing authorization material.
- Confirm no repair, teardown, or post-logout recovery is pending at the start
  of each isolated journey.
- For each evidence row, record only `PASS`, `FAIL`, or its allowed content-free
  failure category. Never attach terminal output.

## Journey A — clean v0.6.3 install and household lifecycle

1. From an empty isolated installation and state root, run the reviewed v0.6.3
   installer. Confirm it verifies the checksum-bound product archive,
   target-matched standalone verifier, and canonical declaration before the
   executable appears. Confirm `heyfood --version` reports `0.6.3`.
2. Launch the TUI, connect the disposable account, and complete owner
   onboarding if requested. Run `/household`; confirm the owner and current
   context agree with the TUI chrome.
3. Run `/household add`. Complete relationship, display label, age band, and
   all eight version-1 dietary-profile steps. Review and save.
4. Confirm exactly one success appears only after the member and complete
   declared profile commit, the new member becomes selected, and panel/chrome
   agree. Confirm no profile-sync or remote-member consent is requested.
5. Exit normally, relaunch the exact executable for the same account, and run
   `/household`. Confirm roster, completed member profile state, and selected
   member scope survived restart.
6. Submit an ordinary turn while the member is selected. Confirm it fails
   locally with the hosted-context limitation and does not refresh credentials,
   prompt for consent, begin microphone capture, serialize a profile, or make a
   hosted request.
7. Run `/for everyone`. Confirm panel and chrome show `Everyone`, restart the
   TUI, and confirm that scope persists. Submit an ordinary turn and confirm the
   same pre-credential, pre-network failure.
8. Run `/for me`. Confirm panel and chrome return to owner context and an
   ordinary owner turn follows the existing hosted flow.
9. Run `/onboard --for` the eligible existing-member fixture, complete the same
   eight steps, and confirm exactly that existing member is updated without
   creating another roster entry.
10. Start `/household add` again with synthetic draft values, cancel before
    save, and confirm `/household` shows no additional member.
11. Exit normally and confirm alternate screen, cursor, input mode, and terminal
    presentation are restored.

## Journey B — v0.6.2 to v0.6.3 upgrade and current-installer refusal

1. In a new isolated profile, install the immutable public v0.6.2 product and
   connect a disposable account. Launch and exit normally so its supported
   local account state exists.
2. Upgrade with the reviewed v0.6.3 installer. Confirm the old executable
   remains available until the v0.6.3 product, verifier, declaration, checksum,
   and candidate manifest pass verification; then confirm one atomic
   replacement and `heyfood --version` reports `0.6.3`.
3. Launch v0.6.3 and allow native household initialization/migration to finish.
   Confirm `/household` shows the account-bound owner exactly once and no
   repair or imported-snapshot fallback is reported. Add and save one synthetic
   member, restart, and confirm its local declared profile and selected scope
   persist.
4. After the native-state compatibility floor exists, invoke the current
   v0.6.3 installer with `HEYFOOD_VERSION=0.6.2`. Confirm its exact
   supported-version gate refuses the request before any release download or
   executable replacement, leaves v0.6.3 installed, and leaves the floor and
   encrypted household state unchanged. Do not run the archived v0.6.2
   installer or binary: neither knows about the future floor, and either is
   unsupported and unprotected after migration.

## Journey C — authorization rollover without household rebinding

1. In a qualified v0.6.3 profile with a committed synthetic member, select
   `/for me`. Use the content-free test control to expire the application
   session while leaving its authorized refresh path valid.
2. Submit one owner operation. Confirm authority refresh/rollover completes,
   the operation is not replayed blindly, the authenticated account binding is
   unchanged, and `/household` still shows the same local roster and scope.
3. Complete an explicit `heyfood login` authorization replacement for the same
   disposable account. Relaunch and confirm the household repository remains
   bound to that account and its committed scope persists.
4. Select the member, expire authority again, and submit an ordinary turn.
   Confirm the household preflight rejects hosted member guidance before any
   refresh or network work.

## Journey D — logout vault teardown

1. With the synthetic household present, use the content-free test control to
   expire or rotate the disposable account's application session immediately
   before `heyfood logout`; perform no intervening authenticated operation.
   Confirm logout refreshes or reconciles that authority for the same account
   before revocation begins. If the test harness injects an interruption,
   resume the documented logout recovery and confirm it completes the original
   teardown rather than creating a second account or household operation.
2. Confirm hosted cleanup is reported truthfully and local cleanup completes
   even if a controlled remote step is unavailable. Do not record remote
   response content.
3. Using only content-free presence checks, confirm authorization credentials,
   the exact account household key, encrypted vault generations, migration
   guard and other account-bound artifacts, and the completed teardown journal
   are absent. Confirm the global native-state compatibility floor and an
   unrelated non-credential preference or approved legacy non-credential
   fixture remain.
4. Launch `heyfood`. Confirm account connection is required and no prior owner,
   member, profile, or selected scope can be rendered before a new login.

## Content-free result record

| Evidence row | Result | Allowed failure category |
| --- | --- | --- |
| Clean installer verifies product/verifier/declaration |  | `clean_install_verification_failed` |
| Owner panel/chrome agreement |  | `presentation_mismatch` |
| Atomic member and profile save |  | `local_commit_failed` |
| Selected member after save |  | `context_apply_failed` |
| Member scope restart continuity |  | `member_restart_continuity_failed` |
| Member hosted-turn preflight |  | `member_preflight_failed` |
| Everyone selection and restart continuity |  | `everyone_continuity_failed` |
| Everyone hosted-turn preflight |  | `everyone_preflight_failed` |
| Return to owner-hosted context |  | `owner_context_failed` |
| Existing-member onboarding |  | `existing_member_onboarding_failed` |
| Pre-save cancellation |  | `cancellation_failed` |
| Terminal restoration |  | `terminal_restoration_failed` |
| v0.6.2 to v0.6.3 atomic upgrade |  | `upgrade_failed` |
| Native initialization preserves account binding |  | `migration_binding_failed` |
| Current v0.6.3 installer refuses requested v0.6.2 before download |  | `managed_v062_request_refusal_failed` |
| Expired-authority rollover preserves household |  | `authorization_rollover_failed` |
| Same-account login replacement preserves household |  | `authorization_replacement_failed` |
| Member preflight precedes expired-authority refresh |  | `member_refresh_order_failed` |
| Logout resumes and completes |  | `logout_resume_failed` |
| Account key, vault, credentials, and journal absent |  | `vault_teardown_failed` |
| Global native-state downgrade floor retained |  | `downgrade_floor_removed` |
| Rotated-session logout refreshes, resumes teardown, removes vault/key, and retains floor |  | `rotated_session_logout_failed` |
| Unrelated non-credential state retained |  | `teardown_scope_failed` |
| Post-logout launch exposes no prior household |  | `post_logout_isolation_failed` |

The candidate is not release-ready until every row is `PASS`. A failure record
contains only the allowed category, candidate version, and approved digest. It
contains no terminal transcript or household content.

Only after every row passes, set these two protected `native-release`
environment variables to assert the content-free approval:

```bash
gh variable set HEYFOOD_APPROVED_CANDIDATE_RUN_ID \
  --repo frntrllc/heyfood \
  --env native-release \
  --body "$approved_run_id"
gh variable set HEYFOOD_APPROVED_CANDIDATE_SHA256SUMS_SHA256 \
  --repo frntrllc/heyfood \
  --env native-release \
  --body "$approved_manifest_sha256"
```

The tag workflow accepts only that unexpired aggregate artifact from a
successful `workflow_dispatch` run of `.github/workflows/ci.yml` at the exact
tagged `main` commit. It checks the approved manifest digest, complete asset
set, and protected attestations, then attests and publishes those same bytes
without rebuilding. The run ID and digest contain no household evidence and
are not included in the ten public release assets. Do not set either binding
for a failed or incomplete checklist; clear stale bindings after publication.
