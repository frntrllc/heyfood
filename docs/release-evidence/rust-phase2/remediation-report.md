# Rust Phase 2 exact-SHA remediation report

Phase 2 remains **HOLD**. Product commit
`d1b2b92cfd47e9a1e2d60a206e477ca376d008e7` (tree
`c3e72e1a8156bed8a781b02c1de381b3e2ebd195`) is an independently approved
bounded remediation of the six findings raised against merged SHA
`38ad4a68b59cb4dd55a7ae133fa19cf1cc39e466`. It is not a Phase 2 completion,
activation, publication, or cutover claim.

## Corrective product changes

- Authorization and rotating-session state now load through an expected-account
  binding under a durable cross-store marker. New registration initializes both
  stores as one recoverable transaction. Cross-account, missing-half, and
  interrupted states block use before network dispatch.
- The normal macOS/Linux/Windows build enables native credentials by default and
  routes the rotating-session store through the deadline-bounded credential
  broker. Blocking stdin/stdout run concurrently; timeout and early-error paths
  kill and reap the broker. The owner-only session-file store is available only
  on non-Windows through an explicit environment choice or disclosed
  legacy-state compatibility path. `NativeAuthStore` remains the separate
  authority for authorization state and its replacement journal; the staged
  session half crosses the broker while the authorization/journal half does not.
- `log` restores the frozen `Log this meal:` prompt and optional meal-type
  grammar. `item` calls `/v1/channel/tools/explain_item` with channel authority
  and preserves restaurant context. `log --for` builds consent-aware household
  context from account-bound imported state and safe profile reads; `item --at`
  resolves the account-bound imported last search. Unrelated household outbox
  work does not block the selected member; selected-member pending work uses the
  imported local-context fallback without replaying or mutating it.
- One-shot SSE consumption now merges bounded partial text and clarification
  choices into the terminal result using the Python-oracle precedence and stable
  string-list wire shape. Detailed choices retain compatible labels and add a
  structured `choice_details` extension. Human output renders partial-only
  answers, choices, and item-specific fields.
- The parity oracle identifies Python `0.4.0` at exact commit
  `73494a57468dac83b4904ce6c390e36926f5c6fe`, archived by the non-release tag
  `archive/python-cli-73494a57`. An inert tar fixture retains the four reviewed
  sources outside the runtime, and Rust verifies the archive digest and every
  source entry's exact bytes against the frozen manifest.
- `--lat` and `--lng` are canonical again; the long spellings remain aliases.
  Non-finite and out-of-range coordinates fail locally before dispatch.
- `--json completion` returns one typed JSON error instead of shell source.

## Local qualification

The following passed on macOS 26.5 arm64 with Rust 1.94.0:

- `cargo fmt --all -- --check`
- `cargo test --locked --workspace --all-targets`
- `cargo test --locked --workspace --all-targets --no-default-features`
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
- `cargo clippy --locked --workspace --all-targets --no-default-features -- -D warnings`

Focused regressions assert cross-account durable blocking, both initialization
crash boundaries, refusal to restore a stale missing session, brokered native
registration/use through the actual executable, keyring staging, frozen-oracle
log and item request bytes/authority/selectors/character bounds/rendering,
streamed partial/choice merge, coordinate aliases/ranges, and JSON completion.
No production canary or production converse request was run by this remediation.

Hosted exact-head qualification passed:

- Rust CI run `29886443450`: 39 successful jobs and one expected conditional
  provenance skip.
- Native CLI CI run `29886443453`: 6 of 6 successful jobs.

The Rust specialist returned GO for the six-finding remediation at the exact
product commit and tree above. That verdict is not Phase 2 exit or merge
approval.

The same specialist returned GO for evidence commit
`b13c81fe5fb0f62677c8aa83f0bf40d034e12e8c` (tree
`15cd1f89ac0456d10ca88b593c146bbdbd3abf46`). That verdict approves the
evidence content only and is also not Phase 2 exit or merge approval.

## Remaining Phase 2 blockers

- Twenty-one root command variants remain explicit placeholders and classic chat is
  absent.
- Full household/location/conversation parity and leaf renderers remain
  incomplete.
- Grocery weekly, exclusions/never, explicit screening, stable item-index cache,
  secure file export, DG-R2 evidence, and the authorized non-destructive canaries
  remain incomplete.
- Health authorization completion polling and canaries remain incomplete.

## Release-boundary incident

Public immutable GitHub Releases `v0.4.0` and `v0.4.1` already exist despite the
approved prohibition on Phase 2 publication and supported-product cutover. They
target `b26174322ba5b0da79946ca879eb0118a5acbabd` and
`9f3b5ad9683fdff9211e3fc077c1ac01c4916896`, contain downloadable native
artifacts, and have recorded downloads. This remediation did not create,
modify, or remove those releases. The owner/release team must classify and
reconcile them before any publication or cutover claim.

The draft remediation PR must remain unmerged. Grocery activation, provider
token storage, Kroger binding, Phase 3, publication, and product cutover remain
prohibited.
