# Releasing heyfood

Only FRNTR, LLC maintainers authorized for the protected `pypi` GitHub
environment may publish heyfood.

## Release prerequisites

1. All P0 gates in the execution plan are satisfied on the default branch.
2. CI passes for every supported Python version on macOS and Linux.
3. The version in `src/heyfood_cli/__init__.py` is the intended release version.
4. `CHANGELOG.md` moves relevant entries from `Unreleased` into a dated version
   section and includes breaking-change migration notes.
5. Wheel and sdist metadata, contents, installation, help, and uninstall smoke
   tests pass from a clean environment.
6. The release commit contains no secrets, private data, or proprietary service
   content.

## Publication contract

- Tags use `vMAJOR.MINOR.PATCH` and must resolve to the reviewed release commit.
- The release workflow is the only supported publishing path.
- Publication uses PyPI trusted publishing through the protected GitHub
  environment. Do not create or store a long-lived PyPI API token.
- The workflow builds artifacts once, verifies them, and publishes those exact
  bytes. It must not rebuild between verification and publication.
- GitHub release notes and the changelog must describe the same version.

Before the first release, register a pending trusted publisher on PyPI with
these exact values:

- PyPI project: `heyfood-cli`
- GitHub owner: `frntrllc`
- GitHub repository: `heyfood`
- Workflow: `release.yml`
- Environment: `pypi`

The protected `pypi` environment must require maintainer approval. The publish
job alone receives `id-token: write`; builds and tests have read-only repository
permissions. Workflow actions are pinned to reviewed commit SHAs, and the PyPI
publisher emits attestations for the verified wheel and sdist.

## Release procedure

1. Merge the reviewed version/changelog commit after CI succeeds.
2. Create an annotated `vMAJOR.MINOR.PATCH` tag at that exact commit and push
   only the tag.
3. Review the protected `pypi` deployment request and confirm its commit and
   artifact checks before approval.
4. Verify the PyPI project page, attestations, GitHub release attachments, and
   a fresh `pipx install heyfood-cli` on macOS and Linux.
5. If any verification fails, follow the fix-forward policy below; do not
   rebuild or replace the published version.

## Failed or unsafe releases

PyPI filenames and released versions are immutable. Never delete and silently
replace an artifact under the same version.

- If publication fails before PyPI accepts any artifact, correct the workflow
  and rerun only when the exact version remains unused.
- If any artifact was accepted, treat the version as consumed. Fix forward with
  a new patch version.
- If a published version is broken or unsafe, yank it on PyPI, publish a GitHub
  advisory or release warning as appropriate, and ship a corrected version.
- If credentials or provenance are suspect, stop publication, preserve logs,
  rotate affected credentials, revoke sessions where applicable, and follow
  the security policy.

The hosted hello.food service has its own deployment and rollback process.
Rolling back the service does not change or replace an already published CLI
artifact; compatibility must be restored additively or through a new CLI
release.
