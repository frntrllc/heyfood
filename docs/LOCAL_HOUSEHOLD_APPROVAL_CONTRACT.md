# Local household approval contract

**Contract:** `local_household_approval_protocol_v1`
**Status:** Phase 0 frozen; not routed by the public v0.7.0 CLI, manifest, or MCP server

This contract governs future local household changes prepared for a coding
agent and reviewed by a person in the attached heyfood TUI. It is separate
from the hosted Grocery approval protocol. Nothing in an agent message, MCP
argument, host permission dialog, redirected stream, or automated terminal
session can approve or commit a household change.

## Authority split

An agent may read only currently disclosed household projections, prepare a
typed change, inspect privacy-filtered status, cancel before commit dispatch,
and request reconciliation. The person supplies protected profile answers,
reviews the complete exact change, and chooses `Save changes` or `Cancel` in
the attached TUI. There is no agent confirm or commit tool. Permanent erasure
is not a v0.8.0 operation; archive is the recoverable removal action.

The agent-visible proposal reference is lookup authority only. Agent-visible
documents never contain the account binding, vault path or key, lifecycle
generation, reducer commit ID, effect fingerprint, proposal digest, review
nonce, approval proof, or other commit capability.

## Disclosure grants

Roster identity and minimized declared-profile disclosure are separate,
per-subject local grants. Creating, expanding, or revoking a grant is an
attached-TUI-only action. In v0.8.0 the grant covers every caller running as
the same OS user with access to the same account-bound heyfood state; it does
not claim to distinguish Codex, Claude Code, OpenClaw, or another process.

An adult profile grant requires the account owner to affirm that the member
authorized disclosure to those local callers. A guardian may grant roster
visibility for a minor, but a minor's profile is not agent-readable. Unknown
age also fails closed for profile disclosure. `Everyone` is all-or-nothing for
the requested projection. Revocation advances the disclosure generation,
invalidates affected drafts, destroys active intake bindings, and downgrades
later status to content-free output. It cannot recall values already returned
to another process or model provider.

## Proposal lifecycle

The legal transitions are:

```text
prepared -> awaiting_local_input | awaiting_local_review
awaiting_local_input -> awaiting_local_review
prepared | awaiting_local_input | awaiting_local_review
  -> cancelled | expired | stale | rejected
awaiting_local_review -> committing
committing -> committed | reconciliation_required
reconciliation_required -> committed | proven_uncommitted
```

Terminal states are immutable. Default review lifetime is ten minutes.
Account replacement, logout, repair, disclosure revocation, a conflicting
household/profile/scope revision, expiry, or lifecycle-generation change
invalidates the draft.

Every edge above is a typed journal compare-and-swap and is enumerated as a
closed, unique set in the protocol schema. Scope presentations remain
content-free, but their affected-subject roster grants are still required and
revalidated independently of that output projection.

Add and profile edit begin without an effect fingerprint. Heyfood allocates
the proposal, reducer commit, and any new member identity before local intake,
but freezes the semantic timestamp, exact before/after hashes, proposal
digest, and repository effect fingerprint only after all protected input is
complete and valid. Freezing those values, advancing the proposal generation,
and changing to `awaiting_local_review` is one durable compare-and-swap.

`Save changes` reacquires the lifecycle lock and revalidates the account,
disclosure generation, household revision, member/profile revision, proposal
generation, digest, and expiry. The winning compare-and-swap changes
`awaiting_local_review` to `committing`. Cancellation is known non-mutating
only when its compare-and-swap wins first. After `committing`, cancellation
returns `household_cancel_too_late` and status must reconcile.

## Exact-once commit and recovery

The household reducer reuses its applied-commit ledger. The preallocated
commit/member identities, frozen effect fingerprint, exact household delta,
new revision, and applied-commit record are published atomically. If status
persistence is interrupted after repository publication, recovery reads that
co-committed record; it never allocates a second identity or blindly repeats
the mutation.

The proof is issued by a repository-held capability bound to the exact
account, proposal, and commit; a public household-state value cannot mint it.
Committed proof requires the matching fingerprint and exact successor
revision. Proven-uncommitted proof requires the authoritative repository to
remain at the pre-dispatch revision with no record for that commit.

An uncertain outcome blocks later household mutation until it becomes
`committed` or `proven_uncommitted`. Preparation, status, cancellation, and
reconciliation have the retry classifications frozen in
`fixtures/agent/household-phase0/dg-r2.json`.

## Human review surface

The inbox is reached through `/household changes` in bare `heyfood`. It is
also discoverable from `/help`, slash completion, the Household panel, and a
content-free pending count. The full future grammar is frozen in
`fixtures/agent/household-phase0/tui-grammar.json`.

The detail view shows every changed and cleared field, who is affected,
disclosure state, scope/conversation consequences, recoverability, and what
remains local. User data is always rendered as data: terminal controls, bidi
controls, and invisible separators are visibly escaped; values cannot create
headings, URLs, instructions, or action labels. The canonical digest remains
bound to validated source values, not escaped display text.

Up/Down moves focus, Enter opens the focused change, Esc returns without a
decision, and Ctrl+C leaves review without product mutation. Final controls
are exactly `Save changes`, `Cancel`, or `Archive member` where appropriate.
While saving, decision controls are disabled and the UI says `Saving
securely…`; it says `Saved` only after exact readback or ledger reconciliation.

Direct human `/household add` preserves the established behavior as one
reviewed transaction: member, complete revision-1 profile, and explicit new
scope are all saved, or all discarded. Agent-prepared Add defaults to no scope
change; a requested bundled scope is likewise all-or-nothing.

## Machine contracts

Phase 0 freezes but does not activate these contracts:

- `schemas/v1/agent-household-read.schema.json`
- `schemas/v1/agent-household-action.schema.json`
- `schemas/v1/agent-household-proposal-presentation.schema.json`
- `schemas/v1/agent-household-outcome.schema.json`
- `schemas/v1/household-agent-disclosure.schema.json`
- `schemas/v1/local-household-approval-protocol.schema.json`
- `schemas/v1/agent-household-native-state.schema.json`
- `schemas/v1/heyfood-agent-compatibility.schema.json`
- `schemas/v3/heyfood-agent-manifest.schema.json`

Their closed synthetic fixtures live under
`fixtures/agent/household-phase0/`. The v3 contract names two local one-shot
reads, six household MCP tools, and the binary-owned offline compatibility
command. Schema v1/v2, their current 30 commands, and the existing six MCP
tools remain unchanged in Phase 0.

## Retention and teardown

Proposal payloads and local intake bindings are encrypted and account-bound.
Cancel, reject, expiry, stale invalidation, successful commit, and
proven-uncommitted reconciliation synchronously remove duplicated protected
content or leave a recovery journal that must finish before another household
read/mutation. A content-free replay tombstone may live for at most 30 days.
It contains no label, member reference, profile value, account identifier, or
repository path.

Logout and account replacement invalidate grants and proposals, destroy
intake bindings, and remove proposal/tombstone data with the exact account's
household key and vault teardown. Incomplete teardown blocks later disclosure.

## Phase boundary

The executable proof in
`crates/heyfood-bin/tests/agent_household_phase0_proof.rs` composes only an
account-bound read and non-mutating prepare/status/cancel path through the
application port. It deliberately creates no CLI command, MCP tool, public
manifest entry, production adapter, TUI automation route, or release claim.
Phase 1 requires separate authorization and exact-SHA review.
