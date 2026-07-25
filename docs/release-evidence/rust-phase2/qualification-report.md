# Rust Phase 2 installed-artifact qualification

The exact product commit is
`7d554209bc889dc707e39c707fdf3a5e820cbddc` (tree
`59d4e619f29e414c4a7eabdae1fe8a0081239355`). The bounded Rust review returned
GO with no findings. Hosted exact-head qualification completed with 46
successful jobs, three expected conditional skips, no failures, and no pending
jobs.

## Installed archive

The unsigned local `aarch64-apple-darwin` archive has SHA-256
`e05dd72e83985ddfca73ee0b252242d4b0cff5f3beea69bc19a95830dc610121`.
The installed executable has SHA-256
`0d38114f1e249b8a472e16733968ee54fb6c767b9362218ab8fcd221176500eb`.

The installed-artifact core matrix passed from the extracted archive, not a
Cargo target. It covered:

- clean-user registration, missing-profile onboarding and consent, profile
  upload, and the first authenticated TUI turn;
- full process exit, native credential reload in a second installed process,
  and a second authenticated turn;
- household-aware Grocery screening, substitutions, proposal editing, cancel,
  single accept, list-version advance, and stale list/context rejection;
- no-blind-retry behavior, stream and pending-confirmation cancellation, and
  terminal restoration;
- semantic behavior at 40, 80, and 120 columns, `NO_COLOR`, and exact archive
  identity.

The unsigned source archive used the explicit isolated-file credential backend.
The final signed-candidate rerun must use the native credential backend on each
platform.

## Production canary

The same exact installed artifact completed native macOS registration against
production. Immediate durable credential readback passed with no reconciliation
marker, closing the fractional-RFC3339-expiry defect found by the first run.
A separate installed process reloaded the Keychain credential, completed
authenticated Grocery and Menu Watch reads, completed a real TUI agent turn,
and exited normally.

Menu Watch and Grocery read qualification passed at the exact head. No Grocery
mutation occurred. The exact-head prepare canary was not repeated because the
immediately preceding production canary had already proven the deployed backend
blocked before confirmation with `confirmation signing key unavailable`, and
the Render configuration remained unchanged. Health was likewise not repeated
at the exact head; the immediately preceding production canary returned HTTP
503 `provider_unavailable`. Health must either be configured and canaried or
remain truthfully deferred from the `0.5.0` support contract.

Canary cleanup revoked one link, two sessions, and two devices. The postflight
session probe returned HTTP 401. Both isolated native Keychain entries and all
isolated local credential files were removed.

## Remaining release path

The protected signing workflow source is qualified, but no signing candidate
has run. The GitHub environment still lacks the macOS Developer ID/notary and
Windows Authenticode inputs, and no corresponding signing material was found in
the authenticated production or management AWS accounts.

The shortest remaining path is:

1. Configure the deployed Grocery confirmation-signing secret and run the
   positive, cancel, conflict, and non-mutation canaries.
2. Configure and canary Health, or preserve its explicit `0.5.0` deferral.
3. Provision the protected macOS and Windows signing inputs.
4. Build signed/notarized candidates and rerun the installed-artifact matrix
   with native credential backends on every platform.
5. Freeze the exact candidate lineage and obtain independent exact-SHA and
   exact-archive approval.

PR #27 remains draft and unmerged. The installer remains fail-closed, the
release workflow remains disabled, and no tag or release was published.
