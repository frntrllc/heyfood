# Dependency, platform, and trust baseline

This Phase 0 baseline records the support and installer-trust design that later
release phases must implement and qualify. It does not claim that a signed GA
artifact or the protected release environment already exists.

## Supported platform floor

| Platform | Minimum | GA targets | Native dependencies and fallback |
|---|---|---|---|
| macOS | macOS 13 | `aarch64-apple-darwin`, `x86_64-apple-darwin` | Keychain/Security framework; CoreAudio for native voice. Typed input remains available when microphone permission or audio is unavailable. |
| Linux | glibc 2.28 | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | AlmaLinux 8/manylinux-class build baseline; Secret Service requires a D-Bus session; native voice declares its ALSA/Pulse/PipeWire bridge. Missing services must fail boundedly to owner-only credentials and typed input. Musl is not a GA claim. |
| Windows | Windows 11 23H2 or Windows Server 2022 | `x86_64-pc-windows-msvc` | ConPTY, Credential Manager, owner-only DACLs, and the selected native audio backend. Windows ARM64 is not a GA claim. |

The Rust toolchain floor is `1.94.0`. Reqwest uses Rustls; OpenSSL and
native-TLS families are denied. `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, and a
future explicit `HEYFOOD_CA_BUNDLE` contract require target-native release
qualification before GA.

## Evidence ownership

FRNTR Release Engineering owns build reproducibility, real-hardware install,
auth, TUI, credential, typed-turn, and upgrade smoke evidence for every GA
target. The Security release approver owns dependency policy, OIDC identity,
Sigstore/Rekor verification, platform-signature verification, and promotion.
The Product release approver owns accessibility and functional-parity
acceptance. A release is blocked when any target lacks an identified operator
and captured evidence; cross-compilation or a hosted runner alone never
substitutes for the required real-hardware RC smoke.

## Pinned release identity design

| Claim | Required value |
|---|---|
| Protected GitHub environment | `native-production-release` |
| OIDC issuer | `https://token.actions.githubusercontent.com` |
| OIDC audience | `sigstore` |
| OIDC subject | `repo:frntrllc/heyfood:environment:native-production-release` |
| Repository claim | `frntrllc/heyfood` |
| Workflow identity | `https://github.com/frntrllc/heyfood/.github/workflows/rust-release.yml@refs/tags/v<VERSION>` |
| Accepted workflow regex | `^https://github\\.com/frntrllc/heyfood/\\.github/workflows/rust-release\\.yml@refs/tags/v[0-9]+\\.[0-9]+\\.[0-9]+$` |
| Ref policy | protected `refs/tags/v<VERSION>` only; branch and pull-request identities fail |
| Provenance subject | exact release artifact name plus SHA-256 from the canonical manifest |

The protected environment requires the Security and Product release approvers;
untrusted pull-request jobs cannot access signing credentials or promotion.
Identity rotation requires a reviewed dual-authorized transition manifest, the
old and new identities, a monotonic sequence, and an explicit rollback floor.

The installer verifies RFC 8785/JCS canonical manifest bytes, the pinned OIDC
identity, artifact size and SHA-256, offline Rekor inclusion proof/integrated
time, DSSE/SLSA provenance subject, and the relevant Developer ID/notarization
or Authenticode chain before atomic installation. It never downloads a dynamic
`cosign` binary or accepts an arbitrary artifact URL.

## Phase boundary

Phase 0 freezes this design for specialized architecture/security review.
Creating the protected environment, provisioning Apple/Windows credentials,
implementing `rust-release.yml`, capturing real-hardware evidence, and shipping
the native installers remain later release gates; they are not prerequisites
for provider-neutral Phase 1 code.
