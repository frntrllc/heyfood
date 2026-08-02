# Native household local state

The v0.7.1 native hey.food TUI keeps its household roster, selected scope, and
declared dietary profiles in the account-bound encrypted household repository.
Local roster management, member onboarding, and persistent scope selection are
part of the supported v0.7.1 TUI contract. The TUI does not use the imported
Python snapshot after native activation.

## Supported human workflow

In `NativeEnabled` mode, an attached human TUI supports:

- `/household` for the live roster and current target;
- `/household add` for one atomic member-plus-profile creation;
- `/onboard --for <exact member ID or exact display name>` for an eligible
  existing member;
- `/for me`, `/for <exact member ID or exact display name>`, and
  `/for everyone` for persistent context selection.

The member questionnaire is the complete version-1 declared-profile
questionnaire used by owner onboarding. New-member creation stores the member,
revision-1 declared profile, and selected scope in one repository commit.
Existing-member onboarding updates only that member and creates no remote-sync
outbox entry. Duplicate display names require an explicit stable-ID-bound
choice.

Edit, archive, restore, and permanent member erasure are not yet available in
the native TUI.

## Privacy boundary

Non-owner profiles are persisted only on this device. Adding or onboarding a
member does not grant profile-sync consent, create a remote member profile, or
infer permission from account ownership or relationship.

When a member or `Everyone` is selected, ordinary hosted guidance and
evaluation acquire the exact live encrypted snapshot under a revision-bound
read lease. The selected declared-profile projection is sent as transient
request context: one profile for a member, or the owner plus every eligible
active member for `Everyone`. The client intentionally omits the top-level
server-synced `household_scope` so request-first local profiles remain the
authority. `/for me` restores the owner context. Invalid, archived, incomplete,
conflicted, stale, or cancelled contexts fail before HTTP dispatch.

For this slice, a member “dietary graph” means only their complete declared
version-1 local profile and evaluation derived from that declared context. It
does not include learned preferences, history, goals, health or fitness data,
cross-device roster sync, remote member profile sync, or remote erasure. Those
capabilities remain deferred behind a separate hosted privacy and consent
contract.

Household lifecycle mutation is human-TUI-only. It is absent from agent
manifests, one-shot machine JSON, MCP tools, redirected stdin, and command-line
profile arguments.

That statement remains the public v0.7.0 behavior. A future v0.8.0
agent-aware design is frozen separately in
[LOCAL_HOUSEHOLD_APPROVAL_CONTRACT.md](LOCAL_HOUSEHOLD_APPROVAL_CONTRACT.md).
Phase 0 adds contracts and a non-routable fake-port proof only; it does not
change the current human-only surface.

## Account and continuity binding

Every management operation is bound to:

- the current authenticated account;
- the live `HouseholdSession` account;
- the repository state account;
- a reducer mode generation and opaque account-binding digest;
- the expected household revision; and
- one reducer operation/correlation pair.

A mismatch performs no new mutation and no network work. A successful local
commit is not presented as success until the reducer accepts its exact event
and the driver reloads the committed revision, clears old conversation and
subject-bound continuity, and acknowledges the new context. Restart bootstrap
loads the persisted scope from the encrypted repository.

## Cancellation and recovery

Cancellation before the repository boundary creates no commit. After dispatch,
the application reconciles with the original internally allocated commit and
member identities; it never allocates a compensating identity or second
revision. An unprovable result is `OutcomeUncertain` and blocks further work
until a fresh repository bootstrap reconciles visible state.

Legacy compatibility mode has no native mutation authority. Rollback mode can
render repository-backed management state but remains read-only. Repair,
initialization, and teardown states fail closed rather than falling back to an
imported roster.

Logout uses the durable native teardown journal. It resumes interrupted local
cleanup, scrubs known legacy credential locations while retaining
noncredential legacy files, removes the account household key and encrypted
artifacts, and clears local authentication state. Partial or uncertain cleanup
is reported explicitly and remains resumable.

## Release and managed-install boundary

v0.7.0 is the first activated native-state release. Its exact public set is
four product archives, four matching standalone-verifier archives, one
canonical native-state declaration, and `SHA256SUMS`. Both macOS product and
verifier executables are signed and notarized before packaging; all ten assets
are attested and verified after public download.

The immutable v0.6.2 release remains four product archives plus its checksum
manifest and gains no verifier or declaration. A managed v0.6.2-to-v0.7.1
upgrade must preserve the prior executable until the v0.7.1 product,
standalone verifier, declaration, checksum, and native-state floor all verify.
After the floor exists, the current v0.7.1 installer invoked with
`HEYFOOD_VERSION=0.6.2` must fail at its exact supported-version gate before
download or executable replacement and must leave v0.7.1 and local state
unchanged. The archived v0.6.2 installer and binary do not know about the
future floor; executing either after migration is unsupported and unprotected.

## Diagnostic handling

Household names, stable IDs, age evidence, profile answers, selectors, account
digests, correlations, credentials, and vault data are excluded from Debug,
logs, errors, panic diagnostics owned by this feature, and machine-readable
history. The bounded text shown inside the user's attached TUI is the explicit
presentation exception.

Release qualification uses the content-free clean-install, upgrade,
downgrade-floor, authorization-rollover, lifecycle, and logout-teardown
[native household TUI manual acceptance](HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md)
checklist. Household lifecycle is not driven through an automated PTY.
