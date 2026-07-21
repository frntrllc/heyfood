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
4. The hosted `https://hey.food/install.sh` and `install.sh.sha256` mirror the
   reviewed release-commit bytes before the tag is pushed.
5. The release commit contains no secrets, private data, or proprietary service
   content.

## Publication contract

- Tags are annotated `vMAJOR.MINOR.PATCH` tags and must resolve to the exact
  current `main` commit.
- `.github/workflows/release.yml` is the only supported publication path.
- The workflow builds on each target architecture instead of cross-compiling:
  `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `aarch64-unknown-linux-gnu`, and `x86_64-unknown-linux-gnu`.
- Each archive is named `heyfood-vVERSION-TARGET.tar.gz` and contains one bare
  regular executable named `heyfood`.
- `SHA256SUMS` covers exactly those four archives. The complete five-file set
  is verified before publication.
- GitHub artifact attestations cover each archive and `SHA256SUMS`. The public
  smoke verifies those attestations before executing a downloaded binary.
- Release assets are immutable. The workflow refuses to publish when a GitHub
  Release already exists for the tag and never rebuilds an existing version.
- The hosted installer accepts an exact `HEYFOOD_VERSION`, independently
  verifies the selected archive, and installs only to an owner-controlled
  directory without `sudo` or shell-profile edits.

## Release procedure

1. Merge the reviewed version/changelog and distribution changes. Wait for all
   required checks on `main` to pass.
2. Deploy the separately reviewed `install.sh` and `install.sh.sha256` bytes to
   `https://hey.food/`, then compare both hosted responses byte-for-byte with
   the current `main` checkout.
3. Create an annotated `vMAJOR.MINOR.PATCH` tag at that exact `main` commit and
   push only the tag.
4. The release workflow validates the tag, runs the native workspace tests,
   builds and smokes all four target executables, creates deterministic
   archives, generates `SHA256SUMS`, attests all five files, and creates the
   GitHub Release.
5. The reusable post-release workflow downloads the public files on all four
   target runners, verifies checksums, archive policy, and GitHub attestations,
   then runs both the public executable smoke and the hosted installer contract.
6. Confirm the GitHub Release and all four public smoke jobs are green. A
   release is not complete while any target or hosted-installer smoke is red.

## Failed or unsafe releases

Do not replace, delete, or silently rebuild published assets under the same
version.

- If failure occurs before a GitHub Release is created, correct the issue and
  rerun only if no assets for that version were published.
- If the GitHub Release exists, treat the version as consumed. Mark it clearly
  as broken or prerelease as appropriate and fix forward with a new patch
  version.
- If provenance or credentials are suspect, stop publication, preserve logs,
  rotate affected credentials, revoke sessions where applicable, and follow
  the security policy.

The hosted hello.food service has its own deployment and rollback process.
Rolling back the service does not change or replace a published CLI artifact;
compatibility must be restored additively or through a new CLI release.
