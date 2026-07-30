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
5. A reviewed `frntrllc/hellofood` coordination PR is ready to update the
   canonical website release manifest, hosted installer and checksum,
   provenance, landing-page metadata, and all public documentation to the exact
   release version and source commit.
6. The release commit contains no secrets, private data, or proprietary service
   content.

## Publication contract

- Tags are annotated `vMAJOR.MINOR.PATCH` tags and must resolve to the exact
  current `main` commit.
- `.github/workflows/release.yml` is the only supported publication path.
- The workflow builds on each target architecture instead of cross-compiling:
  `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `aarch64-unknown-linux-gnu`, and `x86_64-unknown-linux-gnu`.
- Windows distribution remains deferred to a separately qualified future
  release. Ordinary Windows compile,
  test, Clippy, credential-backend, and deterministic packaging CI remains
  required, but no Windows asset or signing credential participates in the
  macOS/Linux publication path.
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
2. Create an annotated `vMAJOR.MINOR.PATCH` tag at that exact `main` commit and
   push only the tag.
3. The release workflow validates the tag, runs the native workspace tests,
   builds and smokes all four target executables, creates deterministic
   archives, qualifies embedded agent discovery, reversible Codex/Claude setup,
   and the bounded MCP protocol, generates `SHA256SUMS`, attests all five
   files, and creates the GitHub Release.
4. The initial post-release workflow downloads the public files on all four
   target runners, verifies checksums, archive policy, and GitHub attestations,
   then installs the exact new version through the reviewed repository
   installer. The hosted default continues serving the prior supported release.
5. Only after step 4 is green, merge the prepared `frntrllc/hellofood`
   coordination PR. It must update `website/src/components/heyfood/release.json`,
   the exact `install.sh` and checksum bytes, installer provenance, landing-page
   metadata, and public documentation in one change.
6. Compare `https://hey.food/install.sh`, its checksum and provenance,
   `https://hey.food/`, and `https://hey.food/docs` with the reviewed website
   source. Manually dispatch the hellofood
   `heyfood release coordination` workflow and the reusable heyfood
   post-release workflow with hosted-installer verification enabled.
7. Confirm the GitHub Release, all four public artifact smokes, all four
   hosted-installer smokes, and the cross-repository version-coordination
   workflow are green. A release is not complete while any target, installer,
   landing-page, documentation, or source-provenance check is red.

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
