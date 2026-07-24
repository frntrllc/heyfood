# Native release signing

The tag-driven release workflow uses the protected `native-release` GitHub
environment. A release build fails closed when any required signing input is
missing, when a signing identity differs from the protected publisher identity,
when timestamping or notarization fails, or when platform verification rejects
the resulting executable.

## Protected environment configuration

Configure these secrets in `native-release`:

- `MACOS_DEVELOPER_ID_P12_BASE64`
- `MACOS_DEVELOPER_ID_P12_PASSWORD`
- `APPLE_NOTARY_ACCOUNT`
- `APPLE_NOTARY_APP_PASSWORD`
- `WINDOWS_CODESIGN_PFX_BASE64`
- `WINDOWS_CODESIGN_PFX_PASSWORD`

Configure these environment variables:

- `APPLE_DEVELOPER_TEAM_ID`
- `WINDOWS_CODESIGN_SUBJECT`
- `WINDOWS_TIMESTAMP_URL`

The macOS P12 must contain exactly one `Developer ID Application` identity.
Both macOS architectures are signed with hardened runtime and a secure
timestamp, submitted with `notarytool --wait`, required to return `Accepted`,
and assessed by Gatekeeper before packaging. Signing, packaged-archive smoke,
and downloaded-public-artifact smoke each require the executable's exact
`TeamIdentifier` to match `APPLE_DEVELOPER_TEAM_ID`.

The Windows PFX must contain a private key with the Code Signing enhanced key
usage. The certificate subject must exactly match `WINDOWS_CODESIGN_SUBJECT`.
SignTool uses SHA-256 for the file and RFC 3161 timestamp digests. Both the
packaging smoke and public artifact smoke require a valid trusted signature,
the expected publisher subject, and a timestamp certificate.

## Protected candidate qualification

Ordinary pull-request CI builds unsigned platform fixtures and tests archive
determinism. It cannot satisfy the signing gate.

Before merge or publication, dispatch `Native CLI CI` with
`qualify_signed_candidate=true` at the exact proposed product SHA. The
`native-release` environment builds the five archives without creating a tag
or GitHub Release, requires the protected macOS and Windows identities,
attests each archive, and reruns the bounded installed-artifact matrix with
Keychain, Secret Service, or Credential Manager. macOS uses a disposable
qualification Keychain and records its destruction as separate evidence so
credentials are not left in the runner's login Keychain.

Each protected build job runs the per-archive smoke gate because it owns
exactly one target archive. The publication and public-download jobs separately
run the strict complete-set verifier, which requires all five archives and the
canonical `SHA256SUMS` manifest. A per-target job therefore cannot weaken or
accidentally invoke the complete publication-set assertion.

Candidate evidence remains incomplete until all five protected jobs pass and
an independent reviewer approves the exact product SHA and archive digests.
Release evidence remains incomplete until the subsequently published,
downloaded artifacts pass the post-release platform checks.
