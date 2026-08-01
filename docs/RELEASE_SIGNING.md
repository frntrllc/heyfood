# Native release signing

The `v0.6.3` tag-driven release workflow uses the protected `native-release`
GitHub environment. Its exact public set is four `heyfood` product archives,
four matching `heyfood-installer` standalone-verifier archives, one canonical
native-state declaration, and one `SHA256SUMS` manifest. The four targets are
macOS Apple Silicon, macOS Intel, Linux ARM64, and Linux x64.
Windows distribution is deferred to a separately qualified future release;
ordinary Windows CI remains
required, but the `v0.6.3` protected candidate, publication, and public-smoke
paths consume no Windows signing credential and emit no Windows asset.

## Protected `v0.6.3` environment configuration

Configure these secrets in `native-release`:

- `MACOS_DEVELOPER_ID_P12_BASE64`
- `MACOS_DEVELOPER_ID_P12_PASSWORD`
- `APPLE_NOTARY_ACCOUNT`
- `APPLE_NOTARY_APP_PASSWORD`

Configure this environment variable:

- `APPLE_DEVELOPER_TEAM_ID`

The macOS P12 must contain exactly one `Developer ID Application` identity.
The product and standalone-verifier executables for both macOS architectures
are each signed with hardened runtime and a secure timestamp, submitted with
`notarytool --wait`, required to return `Accepted`, and checked as notarized
standalone code with Apple's `codesign` notarization requirement before
packaging. Signing, packaged-archive smoke, and downloaded-public-artifact
smoke each require both executables' exact `TeamIdentifier` to match
`APPLE_DEVELOPER_TEAM_ID`.

The Linux archives do not require a platform code-signing identity. The exact
bytes of all eight archives, the declaration, and the canonical `SHA256SUMS`
manifest are covered by GitHub artifact attestations. Public smoke verifies
every attestation and the complete asset boundary before executing the product
and verifier for its target.

The immutable v0.6.2 asset set remains exactly its released four product
archives and checksum manifest. Native-state source does not add verifier or
declaration assets to that historical release. v0.6.3 is the first activated
native-state release and must keep the workspace version, `SUPPORTED_VERSION`,
and `NATIVE_STATE_RELEASE_VERSION` aligned. The compatibility details are in
[`NATIVE_STATE_COMPATIBILITY.md`](NATIVE_STATE_COMPATIBILITY.md).

## Protected candidate qualification

Ordinary pull-request CI includes Windows and builds unsigned platform fixtures
to test compilation, Clippy, credentials, installed behavior, and archive
determinism. It cannot satisfy the `v0.6.3` protected signing gate.

Before merge or publication, dispatch `Native CLI CI` with
`qualify_signed_candidate=true` at the exact proposed product SHA. The
`native-release` environment builds the four authorized product/verifier pairs
without creating a tag or GitHub Release, attests each pair, and reruns the
bounded installed-artifact matrix with Keychain or Secret Service. macOS uses
a disposable qualification Keychain and records its destruction as separate
evidence so credentials are not left in the runner's login Keychain.

Each protected build job runs the per-archive smoke gate because it owns
exactly one target's product and verifier archives. The aggregate candidate job
then assembles all eight archives, generates the canonical declaration and
nine-entry `SHA256SUMS`, rejects additional assets, verifies the complete
ten-file set, and attests every archive, the declaration, and the manifest.
The tag publication and public-download jobs enforce the same complete-set
policy. Across the four target jobs, public smoke executes every product and
verifier archive; it structurally verifies the declaration and digest-verifies
the complete manifest before execution.

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
a Windows `v0.6.3` release asset.
