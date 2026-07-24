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

## Evidence boundary

Ordinary pull-request CI builds unsigned platform fixtures and tests archive
determinism. It cannot satisfy the signing gate. Signing evidence begins only
when a protected tag build succeeds with the configured identities, and remains
incomplete until the downloaded public artifacts pass the post-release platform
checks.
