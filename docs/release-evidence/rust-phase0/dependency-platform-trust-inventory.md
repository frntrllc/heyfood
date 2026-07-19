# Dependency, platform, and trust inventory

## Recorded

- Rust toolchain floor: `1.94.0`, from workspace metadata.
- TLS client: Reqwest configured with Rustls; Cargo Deny rejects OpenSSL/native-TLS families.
- Dependency advisories/licenses/sources: `cargo audit --deny warnings` and `cargo deny check` are hosted gates; `deny.toml` is the policy source.
- Portable default CI: Ubuntu, macOS, and Windows. Native credential/audio features are separately compiled and tested on macOS and Windows.
- Linux native audio and Secret Service are not claimed by the portable matrix.

## Unresolved prerequisites

- supported minimum macOS, glibc/Linux distribution, and Windows versions;
- Linux ARM64 and macOS Intel build/qualification ownership;
- real Apple Silicon, Intel Mac, Linux x86-64/ARM64, and Windows x86-64 release hardware owners;
- native audio/credential system-library and permission contracts per target;
- exact protected GitHub environment name for signing/promotion;
- Sigstore issuer, subject, audience, repository, workflow, ref/tag, and identity-rotation expressions;
- Apple Developer ID/notarization and Windows signing/SmartScreen ownership.

These missing values are deliberately null-by-policy blockers, not guessed deployment configuration.
