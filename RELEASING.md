# Releasing heyfood

heyfood is released only as an attested native Rust executable through GitHub
Releases. The legacy Python/PyPI channel is not a release authority.

## Release prerequisites

1. The release commit is the current `main` commit and all required native CI
   checks pass.
2. The workspace and `heyfood-bin` versions are the intended release version,
   and `CHANGELOG.md` describes that version.
3. `install.sh`, its macOS/Linux contract suite, `install.sh.sha256`, and every
   release packaging verifier pass.
4. The reviewed installer passes its local exact-version contract. The
   currently supported hosted installer remains unchanged until the new
   release assets exist.
5. A protected `Native CLI CI` workflow-dispatch run with
   `qualify_signed_candidate=true` has succeeded at the exact proposed `main`
   commit. Its unexpired `protected-candidate-release-set` artifact contains
   release-channel binaries and the complete ten-file release set.
6. Every row in
   [`docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md`](docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md)
   is `PASS` against that exact artifact. Only then are the protected
   `native-release` run-ID and `SHA256SUMS`-digest bindings set.
7. A reviewed `frntrllc/hellofood` coordination PR is ready to update the
   canonical website release manifest, hosted installer and checksum,
   provenance, landing-page metadata, and all public documentation to the exact
   release version and source commit.
8. The release commit contains no secrets, private data, or proprietary service
   content.

## Publication contract

- Tags are annotated `vMAJOR.MINOR.PATCH` tags and must resolve to the exact
  current `main` commit.
- `.github/workflows/release.yml` is the only supported publication path.
- Protected candidate qualification builds and signs on each target
  architecture instead of cross-compiling: `aarch64-apple-darwin`,
  `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`, and
  `x86_64-unknown-linux-gnu`. These candidates embed
  `distribution_channel=release` because their exact bytes are the only bytes
  publication may consume.
- The tag workflow never rebuilds, re-signs, repackages, or regenerates the
  approved release set. It downloads exactly one unexpired
  `protected-candidate-release-set` from the protected workflow-dispatch run
  bound in the `native-release` environment, and requires that run to be a
  successful `.github/workflows/ci.yml` run at the exact tagged `main` commit.
- `HEYFOOD_APPROVED_CANDIDATE_RUN_ID` identifies the approved protected run.
  `HEYFOOD_APPROVED_CANDIDATE_SHA256SUMS_SHA256` binds the exact
  `SHA256SUMS` bytes, which in turn bind all nine non-manifest assets. Missing,
  malformed, expired, mismatched, or stale bindings fail before publication.
- Windows CI and distribution are deferred to a separately qualified future
  release. No Windows job, asset, or signing credential participates in the
  v0.9.0 macOS/Linux qualification and publication path.
- Four product archives are named `heyfood-vVERSION-TARGET.tar.gz` and contain
  one bare regular executable named `heyfood`. Four verifier archives are named
  `heyfood-installer-vVERSION-TARGET.tar.gz` and contain one bare regular
  executable named `heyfood-installer`.
- One canonical `heyfood-vVERSION-native-state.json` declaration completes the
  nine checksum-bound assets. `SHA256SUMS` contains exactly nine entries, so the
  complete public set contains exactly ten files.
- Protected qualification attests all eight archives, the declaration, and
  `SHA256SUMS`. Publication verifies those attestations and the complete-set
  boundary before attesting and publishing those same bytes. Public smoke
  repeats checksum, boundary, attestation, and per-target executable checks.
- The required human household acceptance record is represented only by the
  protected run-ID and manifest-digest bindings. It is not a release asset and
  must not contain terminal or household content.
- Release assets are immutable. The workflow refuses to publish when a GitHub
  Release already exists for the tag and never rebuilds an existing version.
- The hosted installer accepts an exact `HEYFOOD_VERSION`, independently
  verifies the selected archive, and installs only to an owner-controlled
  directory without `sudo` or shell-profile edits.

## Release procedure

1. Merge the reviewed version/changelog and distribution changes. Wait for all
   required checks on `main` to pass and record the exact commit.
2. Dispatch `Native CLI CI` at that commit with
   `qualify_signed_candidate=true`. Do not rerun a prior run: use one new run so
   the aggregate artifact name is unique. Wait for all four protected target
   jobs and `protected-candidate-release-set` to pass.
3. Download that run's aggregate artifact, verify all ten protected
   attestations, and calculate the lowercase SHA-256 of its exact
   `SHA256SUMS`. Complete every human-attached journey using the content-free
   transport fixture and procedure in
   [`docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md`](docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md).
4. Only when every row is `PASS`, set
   `HEYFOOD_APPROVED_CANDIDATE_RUN_ID` and
   `HEYFOOD_APPROVED_CANDIDATE_SHA256SUMS_SHA256` in the protected
   `native-release` environment. The artifact must still be unexpired.
5. Create an annotated `vMAJOR.MINOR.PATCH` tag at that exact `main` commit and
   push only the tag.
6. The tag workflow validates the tag and source, fetches the explicitly bound
   artifact from the exact protected run, rejects any workflow/event/SHA/name/
   expiry/digest mismatch, verifies the ten-file and nine-checksum boundary and
   existing attestations, then attests and publishes those same bytes. It does
   not build or mutate release assets.
7. The initial post-release workflow downloads the public files on all four
   target runners, verifies checksums, archive policy, and GitHub attestations,
   then installs the exact new version through the reviewed repository
   installer. The hosted default continues serving the prior supported release.
8. Only after step 7 is green, merge the prepared `frntrllc/hellofood`
   coordination PR. It must update `website/src/components/heyfood/release.json`,
   the exact `install.sh` and checksum bytes, installer provenance, landing-page
   metadata, and public documentation in one change.
9. Compare `https://hey.food/install.sh`, its checksum and provenance,
   `https://hey.food/`, and `https://hey.food/docs` with the reviewed website
   source. Manually dispatch the hellofood
   `heyfood release coordination` workflow and the reusable heyfood
   post-release workflow with hosted-installer verification enabled.
10. Confirm the GitHub Release, all four public artifact smokes, all four
   hosted-installer smokes, and the cross-repository version-coordination
   workflow are green. A release is not complete while any target, installer,
   landing-page, documentation, or source-provenance check is red.
11. Clear the two approved-candidate environment bindings so they cannot be
    mistaken for approval of a later release. Commit/SHA checks already fail
    closed if stale values remain.

## Failed or unsafe releases

Do not replace, delete, or silently rebuild published assets under the same
version.

- If failure occurs before a GitHub Release is created, correct the issue and
  produce and fully requalify a new protected run at the corrected exact
  commit. Never substitute another artifact under an existing approval.
- If the approved artifact expires before publication, dispatch a new protected
  run and repeat exact-byte household acceptance before updating the bindings.
- If the GitHub Release exists, treat the version as consumed. Mark it clearly
  as broken or prerelease as appropriate and fix forward with a new patch
  version.
- If provenance or credentials are suspect, stop publication, preserve logs,
  rotate affected credentials, revoke sessions where applicable, and follow
  the security policy.

The hosted hello.food service has its own deployment and rollback process.
Rolling back the service does not change or replace a published CLI artifact;
compatibility must be restored additively or through a new CLI release.
