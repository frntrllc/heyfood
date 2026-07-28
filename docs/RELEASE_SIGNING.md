# Native release signing

The `v0.6.0` tag-driven release workflow uses the protected `native-release`
GitHub environment. It produces exactly four archives: macOS Apple Silicon,
macOS Intel, Linux ARM64, and Linux x64.
Windows distribution is deferred to a separately qualified future release;
ordinary Windows CI remains
required, but the `v0.6.0` protected candidate, publication, and public-smoke
paths consume no Windows signing credential and emit no Windows asset.

## Protected `v0.6.0` environment configuration

Configure these secrets in `native-release`:

- `MACOS_DEVELOPER_ID_P12_BASE64`
- `MACOS_DEVELOPER_ID_P12_PASSWORD`
- `APPLE_NOTARY_ACCOUNT`
- `APPLE_NOTARY_APP_PASSWORD`

Configure this environment variable:

- `APPLE_DEVELOPER_TEAM_ID`

The macOS P12 must contain exactly one `Developer ID Application` identity.
Both macOS architectures are signed with hardened runtime and a secure
timestamp, submitted with `notarytool --wait`, required to return `Accepted`,
and checked as notarized standalone code with Apple's `codesign` notarization
requirement before packaging. Signing, packaged-archive smoke, and
downloaded-public-artifact smoke each require the executable's exact
`TeamIdentifier` to match `APPLE_DEVELOPER_TEAM_ID`.

The Linux archives do not require a platform code-signing identity. Their exact
bytes and the canonical `SHA256SUMS` manifest are covered by GitHub artifact
attestations and verified before execution.

## Protected candidate qualification

Ordinary pull-request CI includes Windows and builds unsigned platform fixtures
to test compilation, Clippy, credentials, installed behavior, and archive
determinism. It cannot satisfy the `v0.6.0` protected signing gate.

Before merge or publication, dispatch `Native CLI CI` with
`qualify_signed_candidate=true` at the exact proposed product SHA. The
`native-release` environment builds the four authorized archives without
creating a tag or GitHub Release, attests each archive, and reruns the bounded
installed-artifact matrix with Keychain or Secret Service. macOS uses a
disposable qualification Keychain and records its destruction as separate
evidence so credentials are not left in the runner's login Keychain.

Each protected build job runs the per-archive smoke gate because it owns
exactly one target archive. The aggregate candidate job then assembles all four
archives, generates the canonical four-entry `SHA256SUMS`, rejects additional
assets, verifies the complete five-file set, and attests the manifest. The
publication and public-download jobs enforce the same complete-set policy.

Candidate evidence remains incomplete until all four protected jobs and the
aggregate complete-set job pass and an independent reviewer approves the exact
product SHA and archive digests. Release evidence remains incomplete until the
subsequently published, downloaded artifacts pass the post-release platform
checks.

## Deferred Windows release

Windows release packaging, Authenticode signing, and public installer
qualification are deferred together to a future release. The Windows source,
Credential Manager implementation, PowerShell packaging/signing scripts, and
ordinary Windows CI remain in the repository. They do not authorize or produce
a Windows `v0.6.0` release asset.
