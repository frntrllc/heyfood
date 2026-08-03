# heyfood agent-aware household management plan

**Status:** Active `v0.8.0` read-only release execution. Phase 0 contracts and
the Phase 1 household-understanding slice are authorized. Agent household
mutation preparation, approval, commit, and lifecycle activation remain
deferred and must not be advertised.
**Target release:** `v0.8.0`
**Baseline release:** `v0.7.1`
**Baseline source:** `be2db50aef31e19d2addde542ad245c992457b3d`
**Baseline tree:** `c1cd86ab50a55f14017145af0d30777774544859`
**Normative companion contracts:** `docs/HOUSEHOLD_LOCAL_STATE.md`,
`docs/CLI_CONTRACT.md`, `docs/COMMAND_GRAMMAR.md`,
`docs/CAPABILITY_STATUS.md`, `docs/JSON_SCHEMAS.md`,
`docs/AGENT_SAFETY.md`, `docs/AGENT_APPROVAL_CONTRACT.md`,
`docs/NATIVE_STATE_COMPATIBILITY.md`,
`docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md`, and
`docs/plans/2026-07-27-heyfood-agent-native-interface-plan.md`
**Primary users:** people managing local household dietary context and
authorized coding agents such as Codex, Claude Code, and OpenClaw acting under
their control

## Executive decision

Make the supported local household experience agent-aware without giving an
agent silent authority to rewrite another person's dietary safety profile.

`v0.8.0` should expose two related capabilities:

1. **Household understanding:** agents can discover the account-bound local
   roster, active scope, profile readiness, and a minimized declared dietary
   profile through typed, read-only CLI and MCP contracts.
2. **Household management:** agents can prepare exact add, edit, archive,
   restore, and scope proposals. In `v0.8.0`, an attached person completes any
   sensitive intake and approves and commits through heyfood's local TUI. The
   agent may observe the result but cannot confirm or commit it.

The agent may gather information, validate it, prepare the exact change,
explain the consequences, wait for approval, and reconcile the outcome. It may
not manufacture missing profile answers, infer consent, treat conversational
agreement as approval, control the TUI, receive commit authority, or retry an
uncertain mutation.

The existing human TUI remains the complete direct-management experience.
Agent integrations call the same application use cases; they do not automate
the TUI or create a second household implementation.

### `v0.8.0` accelerated release boundary

The owner directed an accelerated, same-day `v0.8.0` release on 2026-08-03.
For this release the supported agent surface is deliberately limited to typed,
read-only household understanding: the roster, active scope, and explicitly
authorized minimized profile data for additional household members. The
current account owner's minimized self profile is not claimed by this slice.
Add, edit, archive, restore, scope mutation, proposal preparation, approval,
commit, and TUI automation remain human-only or absent from agent discovery.

Windows distribution and Windows CI are excluded from this release train.
The blocking CI graph runs only on supported macOS and Linux hosts;
platform-neutral format, contract, provenance, migration, and evidence checks
run once on Linux. The deferred native-audio vertical is removed from ordinary
release-blocking CI. It remains separately qualifiable before any future voice
support claim.

Qualification builds the four supported archives once. The protected macOS
signing/notarization and Linux attestation outputs are the exact bytes used for
installed-artifact qualification, review, and publication; the release process
must not rebuild equivalent artifacts in a later stage.

## Verified `v0.7.0` baseline

The public `v0.7.0` binary currently reports:

- manifest schema version 1 by default;
- an explicitly requested schema-version-2 document with the same public
  command and capability set;
- 30 manifest-listed commands with no household, member, or scope command;
- exactly six local MCP read/discovery tools; and
- `tui_automation: unsupported`.

The local encrypted household repository is a supported human TUI feature.
`/household`, `/household add`, member onboarding, and persistent
Me/member/Everyone scope are live. Agent manifests, one-shot machine JSON, and
MCP intentionally expose no household lifecycle surface. The current Agent
Skill therefore correctly routes household management to the human TUI.

The current native lifecycle is also incomplete for both humans and agents:
member edit, archive, and restore do not exist. This plan implements those
application capabilities before exposing them through an agent adapter;
permanent erasure is explicitly deferred.

### OpenClaw/ClawHub review record — Draft v1

The OpenClaw skill owner independently verified the public `v0.7.0` schema-v1
and schema-v2 documents. Schema v2 is a structural superset: it adds
`native_state_compatibility`, retains byte-identical automation, capability,
and compatibility documents, and retains the same 30 command paths. The
owner initially concluded that the published skill's field gating survived a
future default-schema flip, and skill version 1.0.1 already stopped asserting a
permanent six-tool count. The later independent Draft-v3 review found that
1.0.2 still permits familiar-field duck typing on unknown schema versions;
Draft v4 closes that gap explicitly.

Three findings from that review are accepted:

1. **One published skill serves multiple binary versions.** Household guidance
   must branch on the exact installed manifest and discovered MCP tools. The
   same ClawHub artifact must route `v0.7.0` to the human TUI and use only the
   household operations actually advertised by `v0.8.0`. Flat household
   support or non-support statements are forbidden.
2. **The binary owns compatibility remediation.** A skill may detect an
   incompatible manifest, but it must echo a binary-supplied remedy instead of
   hardcoding installation paths or upgrade syntax.
3. **ClawHub propagation is asynchronous.** A successful publish has taken
   roughly 14 hours to become `latest`. The exact binary candidate must be
   available to the skill owner before skill publication, and public binary
   release must wait for observed ClawHub propagation with explicit schedule
   slack.

These findings do not block Phase 0. Their implementation and qualification
are release gates.

### OpenClaw/ClawHub re-review record — Draft v2

The OpenClaw skill owner returned GO with no objections after verifying the
revised compatibility design. The re-review adds three binding clarifications:

1. The currently published skill version 1.0.2 contains an unconditional
   statement that local householding is human-TUI-only. That statement remains
   correct for `v0.7.0`, but the shared-skill update must replace and restructure
   that household section around manifest/tool discovery. Appending `v0.8.0`
   guidance while retaining the flat statement is not acceptable.
2. The same conditional skill is public before the `v0.8.0` binary. Therefore,
   the public `v0.7.0` path is a primary qualification case, not a reduced
   regression case. It runs first and receives the complete absent-capability,
   truthful-handoff, and fail-closed matrix.
3. The OpenClaw owner begins the skill implementation only from an exact signed
   Phase 1 candidate and derives every household claim from that executable.
   Final release qualification repeats discovery against the final exact
   candidate if any manifest, command, schema, or MCP bytes change later.

The owner will build one artifact against both binaries and will not publish a
household claim that has not been observed from the corresponding binary.

### Independent specialist review record — Draft v3

Five independent tracks reviewed Draft v3 against source contracts. None found
a reason to abandon the program, but each withheld plan GO until its findings
were binding. Draft v4 adopts these decisions:

- **Rust:** use a separately versioned local household approval protocol, a
  linearizable cancel/commit state machine, preallocated identities, the
  existing co-committed applied-commit ledger, and sibling adapter topology.
- **Security/privacy:** require per-subject agent disclosure consent, keep exact
  review/profile data local, defer permanent erasure, freeze native-state
  migration/downgrade behavior, and bound hostile rendering and retention.
- **CLI/TUI:** freeze the command/tool matrix, preserve explicit human Add scope
  behavior, make the inbox and direct lifecycle grammar discoverable, and
  expand attached-human installed-artifact acceptance.
- **Agent/OpenClaw:** make schema v3 mandatory, reject unknown-schema duck
  typing, add a version-invariant binary compatibility bootstrap and embedded
  skill identity, qualify stale/pinned skill cases, and keep roster/profile,
  lifecycle, and local-evaluation claims distinct.
- **Release:** bind final qualification to the exact eventual main/tag SHA and
  protected ten-file aggregate, qualify v0.7-to-v0.8 migration, restart the
  ClawHub cycle after any byte change, cut over and verify the hosted installer
  in the correct order, and use an explicit content-free evidence allowlist.

### Draft v4 specialist closeout

Exact-text re-review returned GO with no P0–P3 findings from all five tracks:

- Rust verified the local protocol, cancellation CAS, post-intake fingerprint
  freeze, co-committed ledger, sibling adapters, and scoped estimate.
- Security/privacy verified the enforceable all-local-caller boundary, local-
  only sensitive intake, filtered/revocable status projections, local review,
  erasure deferral, retention, hostile rendering, and migration safety.
- CLI/TUI verified exact subject grammar, atomic Add/scope behavior, complete
  inbox-state copy, direct-management discovery, and attached-human evidence.
- Agent/OpenClaw verified mandatory schema v3, frozen v1/v2 views, unknown-
  schema rejection, compatibility bootstrap, stale/pinned skill cases, and
  distinct capability claims.
- Release verified exact-main/tag protected-byte promotion, migration and
  rollback qualification, ClawHub restart rules, hosted-installer ordering,
  four-target smoke, and the content-free evidence allowlist.

These verdicts approve plan text only. Each implementation phase still returns
its own exact product SHA, evidence digest, and independent GO under the review
requirements below.

## Product outcomes

After qualification, a cold agent with only an installed `heyfood v0.8.0`
binary must be able to:

1. discover that local household support is active;
2. distinguish local declared profiles from hosted or synchronized profiles;
3. list eligible members and the currently selected scope without parsing TUI
   output;
4. inspect a minimized declared profile for one exact stable member reference;
5. target a supported read to self, one member, or Everyone without silently
   changing persistent scope;
6. prepare an exact household change or a privacy-preserving local-intake
   handoff without mutating state;
7. hand the person to the heyfood-owned attached-TUI review experience;
8. observe awaiting-input, awaiting-review, committing, committed, cancelled,
   expired, stale, rejected, proven-uncommitted, or reconciliation-required
   status;
9. verify the resulting revision after a successful commit; and
10. stop and reconcile rather than blindly replay an uncertain operation.

The person must be able to understand and control the entire change without
reading an agent transcript. The review must state who is affected, every
field that changes, whether the action is recoverable, what remains local, and
whether the active scope also changes.

## Scope and non-goals

### In scope for `v0.8.0`

- account-bound local roster and scope reads;
- minimized declared-profile reads only after a current per-subject agent
  disclosure grant;
- request-scoped self/member/Everyone targeting for compatible agent reads;
- add-one-member with a complete declared profile;
- edit display, relationship, age evidence, and declared dietary profile;
- archive and restore;
- persistent scope changes;
- agent-safe proposal preparation, status, cancellation, and reconciliation;
- attached-TUI-only sensitive intake, exact review, approval, and commit;
- read-only one-shot JSON fallback where the manifest classifies it
  `agent_safe`;
- responsive human TUI review and direct management parity;
- Agent Skill, OpenClaw, Codex, and Claude Code contract coordination; and
- signed installed-artifact qualification on the supported macOS/Linux release
  matrix.

### Explicitly out of scope

- hosted household creation or synchronization;
- cross-device local household replication;
- remote non-owner profile consent, approval, or erasure;
- a hosted/out-of-band household approval page;
- agent-visible or agent-invoked household confirmation;
- permanent local member erasure in `v0.8.0`; archive is the qualified removal
  operation;
- learned preferences, history, goals, health, fitness, or provider data;
- agent access to raw vault bytes, keys, credentials, account digests,
  repository paths, internal correlations, or commit authority;
- natural-language approval;
- TUI automation as a supported integration;
- Windows distribution, Health, default-build native voice, or provider-token
  work; and
- arbitrary shell, filesystem, URL, or raw backend access through MCP.

## Authority model

Agents are allowed to manage households, but only through a split authority
model:

| Stage | Agent authority | Human authority | Mutation? |
|---|---|---|---|
| Discover | Read manifest/capabilities and only the roster/profile fields covered by current per-subject disclosure grants | Grant or revoke agent disclosure locally for each affected person | No |
| Draft | Supply typed non-sensitive intent; handle profile fields only when a current disclosure grant permits it | Complete local intake for ungranted or protected answers | No |
| Prepare | Ask heyfood to validate and freeze an exact proposal | None; preparation is not approval | No |
| Review | Explain privacy-safe status and provide the bare-heyfood handoff | Inspect the exact local diff and consequences in the attached TUI | No |
| Approve/cancel | Poll or request cancellation before dispatch; cannot approve on the person's behalf | Fresh approve/save or cancel decision bound to the exact proposal | No until approve/save wins the commit CAS |
| Commit | No agent commit authority in `v0.8.0` | Attached TUI invokes one local exact-once application commit | Yes, once |
| Verify | Observe the resulting revision or required reconciliation | May inspect final state in TUI | No additional mutation |

Natural-language messages, MCP arguments, ordinary agent-host permission
dialogs, redirected stdin, form-filling, opaque references visible to the
model, and TUI keystrokes generated by an agent are data—not semantic consent.

### Per-subject agent disclosure and use consent

Account ownership, relationship, a stored local profile, agent-host setup, and
ordinary service authorization do not authorize disclosure of another
person's information to a coding agent or its model provider. Before Phase 1,
heyfood freezes a local, versioned `AgentDisclosureGrant` with independently
switchable scopes for:

- roster identity and relationship metadata;
- minimized profile reads.

Every grant is bound to one exact self/member subject; there is no `Everyone`
grant type. The authoritative grant set is bound to the exact account,
disclosure purpose, allowed data classes, local OS-user/account boundary,
per-subject grant revisions, deterministic revision-set digest, generation,
granting authority, and expiry or revocation state. `v0.8.0` does not claim to
authenticate one coding-agent host against another on a one-shot process call.
A grant therefore authorizes every local caller running with the same OS-user
access to the account-bound heyfood state. The TUI states that boundary plainly
before grant creation and is the only surface that may create, expand, or
revoke it.

The notice also explains that a local caller may send returned values to its
own model/provider, whose processing and retention terms then apply. Revocation
stops future heyfood disclosure but cannot recall copies already disclosed.
The canonical heyfood record remains encrypted and local; that promise never
describes copies a caller has already received. A narrower per-host grant is
deferred until a separately reviewed process-bound identity root exists.

For an adult member, the account owner must affirm that the member authorized
the stated all-local-caller disclosure. For a minor, an authorized guardian
must affirm roster visibility; minor profile reads are not agent-accessible in
`v0.8.0`. An unknown age band cannot receive profile-read permission. All new
or edited profile answers are collected in heyfood's local TUI for `v0.8.0`;
agent-assisted sensitive intake is deferred and no answer from that flow leaves
heyfood.

An ungranted member is omitted from identity-bearing roster results; the agent
may receive only a content-free restricted-member count and a TUI handoff. A
profile read requires both roster and profile-read grants. `Everyone` fails
closed unless every included subject has the required current grant; heyfood
does not return a partial household while presenting it as Everyone. The
application compares the raw `Everyone` projection and its reported count to a
separate account-bound eligible-subject snapshot loaded directly from the
native household repository at the same household revision. The projected
adapter response is never its own completeness authority.

Adapters are not policy authorities. The application layer loads the
authoritative grant set, derives the exact subjects in the returned snapshot,
computes the maximum projection, filters the adapter result, and repeats the
account, purpose, generation, and revision-set check after proposal work. A
malicious or stale adapter returning profile content cannot bypass this
filtering. Minor or unknown-age guardian authority is roster-only; owner-adult
profile authority is invalid for a non-adult subject.

Revocation takes effect before the next read or proposal transition, advances
the disclosure generation, invalidates affected pending proposals and cached
projections, destroys active intake bindings, and blocks status payloads from
repeating protected content. Self/member/Everyone, adult/minor/unknown-age,
grant expansion, revocation, expiry, local-caller, cross-account, logout, and
concurrent-use cases are mandatory fixtures.

## Household operation contract

### Read operations

The read contract exposes the minimum useful local context:

- stable opaque member reference;
- bounded display label;
- owner/member role and relationship label;
- active, archived, incomplete, or conflicted state;
- declared-profile presence, schema version, and revision;
- current self/member/Everyone scope;
- eligible-member count; and
- a minimized declared dietary projection when explicitly requested.

After the exact profile-read grant passes, the minimized profile may include
declared allergies, restrictions, diets, conditions used for food guidance,
avoidances, and completeness evidence. It must exclude credentials, contact
details, account identifiers, vault metadata, internal hashes, learned
history, health-provider data, and fields unrelated to the requested food
workflow. Roster visibility and profile visibility are evaluated separately.

Collection output is deterministic, paginated where needed, and ordered by a
stable non-sensitive key. Duplicate display names never resolve by first match;
the agent must use the returned stable reference.

### Request-scoped targeting

Agent reads should accept an optional exact `subject`:

```text
self
member:<opaque stable reference>
everyone
```

This subject applies only to that operation. It does not change the persisted
TUI scope. Omission uses a frozen validated snapshot of the active scope and
returns that resolved scope in the result. Unknown, archived, incomplete,
ambiguous, stale, or cross-account subjects fail before network dispatch.

### Add

An add proposal preallocates one stable member identity and commit identity.
The agent may prepare roster intent and non-sensitive candidate fields. The
complete declared-profile document is always collected in the attached TUI. It
is never requested, echoed, or returned through the agent conversation in
`v0.8.0`. Heyfood validates every enum, bounded custom value, questionnaire
requirement, and incompatible combination.

Agent-prepared Add does not silently change the active scope. Scope selection
is a distinct proposal operation or an explicitly bundled sub-operation shown
in the same review. Human `/household add` preserves the established `v0.7.0`
result only by showing the new-member scope transition in its final review and
committing member, revision-1 profile, and scope atomically. `Save changes`
commits all three; `Cancel` discards the entire Add and leaves the prior scope
unchanged. There is no partial “save but do not select” control. Agent-prepared
Add defaults to no scope change; if it explicitly bundles a scope change,
`Save changes` commits both or `Cancel` commits neither. A different scope
requires a newly prepared proposal. Success copy states either `Added <label>.
For: <label>` or `Added <label>. Still for: <prior scope>` truthfully. A
duplicate or confusable display label requires explicit stable-reference-aware
resolution rather than silent deduplication.

The review states that the canonical profile remains encrypted and local to
this device, no profile answer from local intake was disclosed to the agent,
and the operation creates neither a remote member nor profile-sync consent. The
person must affirm that they are authorized to store the submitted dietary
information; relationship or account ownership does not infer that authority.

### Edit

An edit proposal is a typed patch against one exact member, disclosure grant,
and profile revision. Protected before/after profile values are available to
the agent only within the current grant; otherwise the TUI collects and renders
them locally while the agent sees status only. Safety-critical values cannot be
erased by omission, generic `null`, or model-generated shorthand. Clearing an
allergy, restriction, or condition is an explicit operation and is visually
distinguished from adding one.

An edit must preserve a complete valid profile or fail without mutation. It
cannot edit the owner through a member route, convert one stable member into
another, or change account ownership.

### Archive and restore

Archive is the ordinary recoverable removal operation. It makes the member
ineligible for active evaluation while retaining the encrypted local record.
If the member is the active scope, the proposal must include the exact scope
transition—normally to self—and commit both changes atomically. Everyone scope
is recomputed from the resulting eligible roster.

Restore returns the same stable member identity. It requires the archived
record and expected revision, and it does not implicitly select that member or
grant synchronization consent.

### Permanent erasure — deferred beyond `v0.8.0`

Permanent erasure is distinct from archive and never appears in the `v0.8.0`
operation enum, manifest, help, skill, or MCP tool schema. Archive is the
release's recoverable removal operation.

A later erasure program must require recent reauthentication and enumerate
every current and previous vault generation, staging file, crash journal,
proposal/approval payload, receipt, export, temporary file, index, and recovery
reference. Cleanup must be crash-resumable, prevent repair or rollback from
resurrecting the member, rotate or destroy affected key material where needed,
and retain only a minimal non-sensitive tombstone. Restart, repair, interrupted
erasure, and attempted rollback must prove forensic absence before any erasure
capability may be advertised.

### Persistent scope

An agent may prepare a persistent scope change, but request-scoped targeting is
preferred for ordinary work. The review shows the previous and next scope and
states that conversation continuity will be cleared. Commit writes the scope
against the exact household revision, reloads it, and resets all subject-bound
conversation and pending-choice state before success.

## Proposal, local review, and commit protocol

### Agent-safe proposal presentation

Preparation and every later status/reconciliation read return a disclosure-
filtered `AgentHouseholdProposalPresentation`, not repository or commit
authority. Its closed projection variants are:

| Current grant | Agent-visible fields |
|---|---|
| No current roster grant | Public proposal reference, operation class, state, timestamps, content-free handoff/reconciliation guidance only |
| Roster grant only | The common fields plus human-readable affected member, stable member reference, non-profile consequences, and recoverability |
| Roster and profile-read grants | The roster projection plus allowlisted before/after profile fields |

Phase 0 deliberately keeps every agent-visible `scope` proposal content-free.
Content-free is an output projection, not authorization: a scope proposal for
an existing subject still requires that subject's current roster grant, and an
`Everyone` proposal binds the complete independently loaded eligible roster.
That subject authority is rechecked independently of projection before the
adapter returns, on status, and immediately before commit.
The exact previous and resulting scope remain in the encrypted local proposal
journal and attached-TUI review; they are not represented as free-form
`ActiveScope` change strings. Archive status may disclose the affected member
only under that member's current grant, while any exact fallback-scope detail
remains local. This avoids turning an untyped diff into a second identity
carrier before a separately reviewed typed scope presentation exists.

Every variant may report `prepared`, `awaiting_local_input`,
`awaiting_local_review`, `committing`, `committed`, `cancelled`, `expired`,
`stale`, `rejected`, `proven_uncommitted`, or `reconciliation_required`.
Immediately before every serialization, heyfood revalidates the account,
projection class, disclosure-purpose-bound grant-set digest, every included
per-subject grant revision, and disclosure generation. A
revocation downgrades all later status and reconciliation results to the
content-free variant; cached or previously serialized profile content is never
repeated.

It never contains the account-binding digest, encryption key, repository path,
internal operation/correlation identifier, lifecycle generation, commit
credential, single-use nonce, approval proof, or any token that can authorize
commit.

### Hidden frozen authority

Internally, heyfood binds the proposal to:

- authenticated account and `HouseholdSession` account;
- encrypted repository account;
- expected household revision;
- exact target member and profile revision, when applicable;
- exact disclosure purpose, per-subject revision-set digest, and disclosure generation;
- exact agent presentation projection class;
- exact proposal reference and operation on every frozen status authority;
- operation and canonical before/after document hashes;
- scope and conversation-continuity consequences;
- originating MCP session and eligible host/version policy;
- expiry;
- preallocated proposal identity, reducer commit ID, and new member ID when
  applicable;
- a proposal/account/commit-specific verifier for a commit-evidence secret
  derived only inside the native repository from a dedicated durable evidence
  root that is preserved across household encryption-key rotation; the
  repository durably reserves the exact proposal/commit tuple before returning
  the verifier, while the proposal journal persists only the one-way verifier;
  repository reopen securely rederives the exact secret and household state
  DTOs cannot mint either proof;
- the frozen effect fingerprint only after every local-input field is complete
  and validated; and
- single-use local review and commit state.

Every compare-and-swap token is also bound to the journal's account, proposal
reference, and reducer commit ID in addition to its revision, state,
generation, and frozen digest. A token from one proposal can never advance or
cancel another proposal even when both journals are otherwise at identical
states. The application allocates the proposal identity before the prepared
authority crosses the outbound port; the port can freeze only that identity
and has no operation that rebinds the authority to an arbitrary proposal.
Durable journal construction accepts only the initial `prepared` or
`awaiting_local_input` states. All later transitions are private journal CAS
operations, so callers cannot transition a bare authority and wrap it after
the fact. The journal exposes typed CAS operations for every edge in the
frozen transition graph, including prepared-to-input, each pre-commit terminal
result, and authoritative `proven_uncommitted` reconciliation.

The encrypted native key-bundle document upgrades from wire version 1 to wire
version 2 inside native-state version 3. A v1 bundle derives the evidence root
once from its authenticated active key during migration; every later rewrite
and finalization preserves that root independently of the rotating encryption
key. The same bounded, account-bound document stores content-free evidence
records for exact proposal/commit reservations using only a domain-separated
proposal-reference hash, commit ID, state, and bounded expiry. A pre-dispatch
cancel, reject, or expiry releases its exact reservation after authoritative
absence is rechecked under the vault lease. Applied reservations are retired
from this auxiliary ledger on the next proposal because the authenticated
applied-commit ledger remains authoritative. Orphaned and denied records expire
after at most 30 days and are pruned before capacity is evaluated; the compact
wire must remain below the credential-broker document ceiling at its maximum
cardinality. Under the vault lease, authoritative absence atomically converts
the matching reservation to a deny record before an unapplied proof is issued.
Until that bounded fence expires, every later dispatch of the exact commit ID
fails closed. If dispatch wins the lease first, absence proof fails and
reconciliation must consume the applied-commit ledger instead.

An `awaiting_local_input` record preallocates identities but does not claim a
final proposal digest or effect fingerprint. Completing local intake validates
the complete semantic candidate, computes and freezes the before/after hashes,
proposal digest, effect fingerprint, and semantic timestamp, advances the
intake generation, and compare-and-swaps to `awaiting_local_review` in one
durable transition. Any later input change creates a new proposal generation
and digest. Crash, cancel, disclosure revocation, and concurrent edit at either
side of this transition are explicit fixtures; none may expose a partially
frozen proposal.

The default proposal review lifetime is ten minutes. Any account replacement,
logout, repository repair, lifecycle generation change, disclosure revocation,
conflicting commit, profile edit, scope change, or expiry makes the proposal
stale or cancelled. A same-generation, same-digest disclosure that expires
during adapter work is still an authority reduction and returns a stale,
content-free proposal rather than a pending one.

### Trusted human review — local TUI only in `v0.8.0`

The approval surface is owned and rendered by heyfood, never by the agent or
untrusted profile content. `v0.8.0` qualifies exactly one path: a pending-change
inbox in the attached heyfood TUI. No local non-owner profile or exact diff is
uploaded to an approval service. The frozen backend-oriented
`AGENT_APPROVAL_CONTRACT.md` protocol v1 remains unchanged and does not govern
this local repository mutation. Phase 0 freezes a separately versioned local
household approval protocol with closed schemas and fixtures; existing v1
proposal or approval schemas are not widened.

The inbox is discoverable through `/household changes`, `/help`, slash-command
completion, a content-free pending-count indicator, and the Household panel.
The proposed Phase-0 grammar is `/household`, `/household add`, `/household edit
<member>`, `/household archive <member>`, `/household restore <member>`,
`/household agent-access <member>`, `/household changes`, and `/for ...`; the
frozen registry, `/help`, and completion inventory must agree exactly. These
commands provide direct human management through the same application use
cases. Up/Down moves focus, Enter opens the focused proposal, Esc returns
without deciding, and Ctrl+C exits the active review without product mutation.
Final actions are explicit focused controls labeled `Save changes` or `Cancel`;
archive uses `Archive member`. The help bar always states the active keys.

The detail view displays every changed, cleared, and scope-transition field,
who is affected, disclosure state, recoverability, and local-only consequences.
The inbox maps every legal protocol state to distinct human copy:

| Protocol state | TUI presentation |
|---|---|
| `prepared` | `Getting this change ready…`; transient but recoverable after restart |
| `awaiting_local_input` | `More information needed`; opens the local intake |
| `awaiting_local_review` | `Ready for your review`; enables `Save changes` and `Cancel` |
| `committing` | `Saving securely…`; controls are disabled while reconciliation owns the result |
| `committed` | `Saved`; shows the truthful scope/result summary |
| `cancelled` | `Cancelled — nothing was saved` |
| `expired` | `Expired — start a new change` |
| `stale` | `Household changed — review a fresh proposal` |
| `rejected` | `Can't use this change`; reserved for policy/authorization/validation refusal, not a human cancel action |
| `proven_uncommitted` | `Not saved — heyfood verified no household change was made` |
| `reconciliation_required` | `Checking whether this was saved…`; later lifecycle mutations stay blocked |

The person can complete the review without access to the agent transcript.
Restart resumes an eligible local draft or shows its truthful terminal state.

Approval records only the exact reviewed proposal. A changed proposal requires
a new review. Cancellation is guaranteed non-mutating only if its compare-and-
swap wins before `committing`. After commit dispatch begins, cancellation
returns `household_cancel_too_late`; status must resolve to `committed`,
`proven_uncommitted`, or `reconciliation_required` and may not claim success.

### Local commit and reconciliation

The attached TUI's `Save changes` action reacquires the lifecycle lock, reloads
the account, disclosure generation, household revision, member/profile
revision, and exact proposal digest, then compare-and-swaps
`awaiting_local_review -> committing`. It reuses the existing household
repository applied-commit ledger: the preallocated commit ID, effect
fingerprint frozen at `awaiting_local_review`, new member ID when applicable,
repository mutation, and applied commit marker are written in one atomic
repository transaction.

After readback, the external receipt is derived from the co-committed marker.
A crash after repository publication but before proposal-status persistence is
reconciled from that ledger using the same identities; it never allocates a
second member or commit. Reconciliation accepts an opaque proof issued only
after the native repository reopens its secure key custody, rederives the
secret bound to that exact account, proposal, and commit, and reads the
authenticated ledger while holding the repository lease. A committed proof
verifies the exact ledger fingerprint and successor household revision. A
proven-uncommitted proof requires the authoritative repository to remain at
the exact pre-dispatch revision with no record for the commit identity. No
proof API accepts public or caller-synthesized household state DTOs, arbitrary
verifier secrets are rejected against the repository-derived binding, and
bare commit/fingerprint pairs are never accepted.
The legal transition table is:

```text
prepared -> awaiting_local_input | awaiting_local_review
awaiting_local_input -> awaiting_local_review
prepared | awaiting_local_input | awaiting_local_review
  -> cancelled | expired | stale | rejected
awaiting_local_review -> committing
committing -> committed | reconciliation_required
reconciliation_required -> committed | proven_uncommitted
```

The approval schema enumerates these 20 edges as a unique closed set and pins
every archived scenario to one legal sequence; duplicate edges, skipped
adjacency, and any terminal-state revival fail validation.

Every transition uses compare-and-swap over the account, proposal digest,
disclosure generation, revisions, state, expiry, and lifecycle generation.
Terminal states are immutable. Pre-dispatch failure is safe to retry only after
a fresh status read. Once dispatch may have crossed the repository boundary,
automatic retry is forbidden and later household mutation is blocked until
reconciliation closes the original operation.

## Proposed machine surfaces

Final names are frozen from application use cases during Phase 0. The intended
shape is:

### Agent-safe one-shot reads

```text
heyfood household show --subject SUBJECT --json --no-input
heyfood household member --member-ref MEMBER_REF --json --no-input
```

`--subject` accepts only `self`, `member:<stable-ref>`, or `everyone`; omission
uses and returns one frozen active-scope snapshot. `household member` is exact
member inspection and does not accept `--subject`. Both commands require
`--json --no-input` for the agent-safe path, emit exactly one schema-validated
ANSI-free JSON value on stdout, reserve stderr for privacy-safe diagnostics,
and return the common documented process exit statuses. They are local reads,
perform no network dispatch, and use the same account-bound repository
controller as the TUI. No one-shot agent mutation command is introduced.

### MCP reads

```text
heyfood_get_household_context
heyfood_get_household_member
```

### MCP protected actions

```text
heyfood_prepare_household_change
heyfood_get_household_change
heyfood_cancel_household_change
heyfood_reconcile_household_change
```

No household confirm tool exists in `v0.8.0`. The person approves and commits in
the attached TUI; the agent may prepare, observe, cancel before dispatch, and
reconcile. Permanent erasure is absent from the preparation operation enum.

### Phase 0 command and tool matrix

Phase 0 freezes the exact grammar and closed input/result/error schemas before
implementation. At minimum:

| Surface | Input | Result evidence | I/O and retry class |
|---|---|---|---|
| `household show` | Closed context-input schema with optional exact `subject`, bounded cursor, and limit | Closed read-result schema: resolved subject, whether it came from active scope, household/disclosure revisions, eligible/restricted counts, and only granted roster/profile projections | Local read; one JSON stdout value; diagnostics on stderr; no retry after state-generation conflict without a fresh read |
| `household member` | Separate closed member-input schema requiring stable `member_ref`; no context selector or cursor | The same closed read-result schema with exact resolved member reference, grant state, profile readiness/revision, and only granted minimized fields | Local read; same stream/exit contract; no display-name resolution |
| `heyfood_get_household_context` | Exact context-input schema only | Semantically identical data and typed errors to `household show` | Local bounded MCP read; cursor snapshot-bound |
| `heyfood_get_household_member` | Exact member-input schema only | Semantically identical data and typed errors to `household member` | Local bounded MCP read |

The matrix also freezes maximum input/output sizes, stable ordering, cursor
rules, unknown/archived/incomplete/ambiguous/cross-account handling, disclosure
denial, and the mapping between typed errors, MCP errors, and process exit
status. Human presentation, if offered, is a separate audience row and never a
machine fallback.

MCP annotations describe actual behavior but do not confer authority. The
adapter exposes no generic command execution, raw repository, raw filesystem,
credential, TUI-control, or arbitrary DTO tool.

## Manifest, schema, and skill compatibility

Agent-manifest schema v1 remains frozen. It cannot gain household fields or
meanings in place.

The public `v0.7.0` schema-v2 structure is also not silently widened. Its
`additive_optional_fields: false` declaration remains authoritative. It cannot
represent the required structured MCP tool/schema/authority inventory, so it
remains frozen as a compatibility view rather than becoming the `v0.8.0`
default.

For `v0.8.0`:

1. schema v3 is mandatory and becomes the default `heyfood agent describe` and
   `agent doctor` document only after shared-skill qualification;
2. explicit `--schema-version 1` and `--schema-version 2` remain available as
   frozen compatibility views and truthfully omit the v3 household surface;
3. schema v3 advertises separately versioned household roster, profile,
   lifecycle, and local-evaluation capabilities plus the exact read/preparation
   commands, MCP tool-set version, authority classes, approval requirements,
   and result schemas;
4. incompatible skills fail closed with an exact binary-owned upgrade
   instruction carried through the manifest contract;
5. the embedded canonical skill and OpenClaw package support the declared
   manifest range before release; and
6. skills derive available operations from the exact manifest/tool list rather
   than asserting a permanent count of six.

### One skill artifact across `v0.7.0` and `v0.8.0`

ClawHub `latest` is one published artifact, not one artifact per heyfood binary
version. Every household sentence and workflow in that skill is conditional on
the exact installed manifest and current MCP tool list:

| Installed contract | Required skill behavior |
|---|---|
| `v0.7.0`, no active household capability or household MCP tool | State that household management is human-TUI-only and hand off to bare `heyfood`; never mention a machine household command or tool |
| `v0.8.0` read capability only | Use only the exact advertised read commands/tools; route changes to the TUI |
| `v0.8.0` prepare/status/cancel/reconcile | Orchestrate preparation and local-TUI handoff; never imply the agent can approve or commit |
| Missing required manifest fields or unsupported schema | Fail closed and echo the binary-owned remediation instruction |

The skill does not branch on release notes, documentation version, or a binary
version string alone. It may not contain a flat statement that local
householding is always human-only or always agent-capable. The same exact skill
candidate is tested against the public `v0.7.0` binary and exact private
`v0.8.0` candidate before publication.

Schema support is checked before field or tool gating. Familiar v1/v2-shaped
fields inside an unknown v3-or-later fixture never authorize duck typing. The
replacement skill declares its exact supported schema range and fails closed
outside it.

The currently published 1.0.2 household section must be restructured, not
extended: its unconditional human-TUI-only sentence becomes the behavior of the
absent-capability branch. No contradictory flat sentence may remain elsewhere
in the skill, its references, examples, or generated package metadata.
Household roster/profile/lifecycle support and local household food evaluation
are distinct capabilities. Enabling the former must not claim the latter; a
negative fixture has roster/profile tools present while local evaluation is
absent.

The independent Draft-v3 review observed ClawHub `latest` at 1.0.2 and recorded
the published `SKILL.md` SHA-256 as
`7315036592d8364a889f066dcba5b9886a34b5a474f34c49716db59a2945376b`.
That digest is evidence for the stale-skill fixture, not a future compatibility
approval.

### Binary-owned remediation

The executable, not the skill, owns compatibility diagnosis and remedy. Phase
0 freezes a version-invariant, offline, credential-free, `agent_safe`
bootstrap command:

```text
heyfood agent compatibility --json --no-input
```

The command remains callable even when the default manifest schema is unknown.
It reads only closed, locally verified setup receipts produced by `heyfood agent
setup`, which bind stable host identifier, host version, skill package version,
and skill digest. Every new skill embeds its exact package/contract version in
frontmatter and package metadata. Caller-supplied model text, host labels, or
`clientInfo` never become trusted identity or mutation authority.

Its separately versioned, closed result reports every discovered managed
installation, supported manifest range, compatibility status, reason, and the
exact binary-owned update/setup command. If no trustworthy receipt exists—as
with a stale manually installed 1.0.2 skill—it returns
`skill_identity_unknown`, no compatibility success, and a safe host-specific
repair command. The skill reads and echoes this result; it does not maintain a
second copy of paths or installation syntax.

Schema v3 carries the compatibility command and structured MCP inventory. The
implementation must not add a field to schema v1/v2, flip
`additive_optional_fields`, overload a descriptive field, or infer compatibility
from familiar fields in an unsupported schema.

The MCP transport protocol may remain at its current version if only typed
tools are added. The household tool-set contract and each result schema receive
their own explicit version. Removing or changing a tool, field meaning,
authority class, or error meaning requires a corresponding contract bump.

The `v0.7.0` skill remains correct for `v0.7.0`: it routes household work to the
TUI. No release note or skill update may retroactively claim an agent surface
for that immutable binary.

### ClawHub publication ordering

The skill owner requires the exact executable, not a plan or release note, to
derive capability claims. Release coordination therefore uses this order:

1. freeze and privately deliver the exact signed `v0.8.0` product candidate,
   embedded manifest/schemas, and MCP tool list to the skill owner;
2. build and test one skill candidate against both public `v0.7.0` and the
   exact private `v0.8.0` binary;
3. publish the compatible skill to ClawHub;
4. independently observe that the intended skill version resolves as ClawHub
   `latest`, then repeat cold resolution from a clean host after propagation;
5. bind that resolved skill digest and compatibility evidence into the
   `v0.8.0` release inventory; and
6. only then authorize merge/tag/publication of the binary.

The former fixed 24-hour scheduling reserve is removed by the 2026-08-03 owner
acceleration directive. This does not waive observed propagation: if ClawHub
has not advanced, the binary waits regardless of elapsed time. Once the
intended skill is independently observed as `latest`, both clean-resolution
checks and exact-candidate qualification may proceed immediately. Publishing
the binary first and accepting a transient false household handoff remains
prohibited.

An exact signed Phase 1 candidate is delivered early so the skill owner can
implement and test the conditional rewrite without waiting for all later
mutation phases. It never receives final release approval/run bindings and is
not the release candidate. Any subsequent change to manifest, command, schema,
MCP, embedded-skill, or compatibility bytes invalidates the earlier derivation
and requires the owner to repeat it against the final signed candidate before
ClawHub publication.

If a defect or byte change appears after skill publication, the release cycle
restarts with a new protected candidate and a new skill package version. The
team repeats the public-`v0.7.0` primary matrix, restarts the 24-hour reserve,
observes two clean `latest` resolutions, and rebinds all digests. Expired
protected artifacts are never promoted. If the binary release is abandoned,
the published skill must remain truthful for `v0.7.0`; its discovered-only
`v0.8.0` branch may remain dormant only if it makes no unconditional claim,
otherwise the skill owner publishes a fix-forward version.

## Architecture and crate ownership

Household agent work follows the existing dependency direction:

```text
heyfood-core
  household commands, patches, proposal presentation, receipts, errors
        ^
heyfood-application
  read, prepare, local-review/commit, cancel, verify, reconcile use cases
        ^                         ^
        |                         |
heyfood-platform          heyfood-cli / heyfood-tui / heyfood-mcp
  outbound port adapters    transport and presentation adapters only
        \                         /
         \                       /
heyfood-bin
  composition only
```

Requirements:

- no business rules in MCP handlers, skill text, Clap dispatch, reducers, or
  the binary composition root;
- CLI, TUI, and MCP use the same application controllers;
- CLI, TUI, MCP, and platform are sibling adapters over application ports;
- presentation adapters never depend on concrete platform implementations;
- repository mutation remains atomic and revision checked;
- renderer-safe local review documents are transport neutral;
- platform code implements outbound ports for encryption, account binding,
  locks, local review storage, and crash recovery;
- MCP never depends on concrete HTTP or repository implementations; and
- diagnostics and evidence remain content free.

## Error contract

Household tools return stable, privacy-safe errors including:

```text
household_unavailable
household_locked
household_account_mismatch
household_state_incomplete
household_state_conflicted
household_member_not_found
household_member_ambiguous
household_member_ineligible
household_profile_incomplete
household_agent_disclosure_required
household_agent_disclosure_revoked
household_revision_stale
household_proposal_expired
household_proposal_cancelled
household_proposal_stale
household_approval_required
household_approval_rejected
household_cancel_too_late
household_outcome_uncertain
household_reconciliation_required
```

Errors may include a public proposal reference, expected next action, retry
class, and whether mutation is known not to have occurred. They never include
raw member profile content, account digests, internal paths, keys, tokens, or
backend error bodies.

## Canonical values and hostile-content rendering

Proposal digests and repository effect fingerprints bind the exact validated
canonical source values, not terminal-wrapped or escaped display text. The
Phase-0 terminal renderer uses one reversible quoted-ASCII encoding. Safe ASCII
is preserved except that quote and backslash are escaped; every non-ASCII,
control, bidi, zero-width, invisible, line-separator, and confusable code point
is emitted as its exact `\u{XXXX}` scalar value. The renderer performs no
Unicode normalization and never substitutes a lookalike. Delimiter characters
that could form a URL or terminal instruction are escaped, so user data cannot
create headings, instructions, keys, URLs, or approval controls. Literal text
such as `<U+001B>` remains distinguishable from an actual escape byte.

- changed and cleared safety fields are never truncated away; compact layouts
  paginate or scroll while preserving the complete focused value; and
- any future HTML surface requires a separately reviewed context-specific
  escaping and content-security contract before activation.

Adversarial terminal-control, bidi, invisible, confusable, markup, URL, long-
line, wrapping, and truncation fixtures must produce the same canonical digest
and an unmistakably data-only presentation at compact, standard, and wide
terminal widths.

## Proposal retention and account teardown

Pending proposal payloads remain encrypted and account-bound. On cancel,
reject, expiry, stale invalidation, proven-uncommitted reconciliation, or
successful commit, duplicated before/after profile payloads and local intake
bindings are synchronously removed before terminal success is reported. If an
interruption prevents cleanup, the teardown/recovery journal resumes it at the
next startup before any household read or mutation.

A content-free replay tombstone may retain only the proposal reference hash,
operation class, terminal state, commit ID/effect fingerprint where applicable,
and timestamps for at most 30 days. It contains no label, member reference,
profile value, account identifier, repository path, or approval content and is
pruned on startup and before new proposal creation.

The native commit-evidence ledger follows the same 30-day ceiling. It never
stores the raw proposal reference. Pre-dispatch terminal cleanup removes a
reservation immediately; crash-orphaned reservations and denied delayed-
dispatch fences are pruned before later reservation capacity is evaluated;
and applied reservations are compacted once the authenticated applied-commit
ledger can replace them. Capacity exhaustion is an explicit fail-closed error,
never an overwrite of unresolved evidence.

Account replacement and logout invalidate every disclosure grant and proposal,
destroy intake/session bindings, purge payloads and tombstones with the exact
account household key/vault teardown, and leave no remote household approval
record because `v0.8.0` creates none. Interrupted or uncertain teardown blocks
new household exposure until the existing journal completes.

## Native-state migration and downgrade contract

Phase 0 freezes whether the household vault schema, proposal journal, applied-
commit ledger, native-state version, required capability set, canonical release
declaration, verifier, and compatibility floor change. Before the first
`v0.8.0`-only write, the managed installer atomically establishes the reviewed
floor and completes or safely rolls back the account-bound migration. An
interrupted migration resumes from its durable journal without exposing a
partially upgraded household.

After activation, an ordinary `v0.7.0` binary or installer is not a supported
writer and must be rejected before download/replacement where the managed
boundary applies. Direct archived execution remains unsupported. A supported
rollback requires a separately qualified binary in rollback-read-only mode; it
must preserve and visibly block unresolved proposal, committing, and
reconciliation journals rather than discarding or interpreting them as older
state.

The installed matrix covers public `v0.7.0` to candidate `v0.8.0`, migration
interruption at every persistence boundary, post-migration restart, requested
managed downgrade refusal, rollback-read-only behavior if offered, account
replacement, logout, repair, and unrelated-state preservation.

## Phased execution plan

### Phase 0 — Contract, threat model, and executable proof

**Purpose:** freeze authority and architecture before feature work.

Deliverables:

1. Inventory the exact `v0.7.0` manifest, command, MCP, household repository,
   and skill boundaries.
2. Freeze operation semantics for add, edit, archive, restore, and scope;
   record permanent erasure as absent from `v0.8.0`.
3. Define minimized read DTOs, proposal presentation, hidden binding record,
   local review record, applied-commit-derived receipt, reconciliation document,
   per-subject disclosure grant, and the complete command/tool matrix.
4. Extend the agent threat model for malicious profile text, confused deputy,
   cross-account access, disclosure without consent, approval spoofing, replay,
   stale revisions, concurrent TUI/MCP changes, cancellation races, and crash
   recovery.
5. Freeze and prototype the attached-TUI inbox grammar, local household
   approval protocol v1, CAS transition table, safe renderer, and direct human
   edit/archive/restore/scope flows. Hosted approval remains out of scope.
6. Prove adversarial fake-port read and non-mutating prepare/status/cancel paths
   from the binary composition root through the application layer, including
   malicious profile replay, wrong-account authority, minor/unknown-age
   downgrade, revocation, and disclosure revision rotation.
7. Produce a DG-R2-style dispatch/retry row for every repository mutation
   boundary.
8. Freeze manifest schema v3, the version-invariant compatibility bootstrap,
   skill identity receipts, and the exact v1/v2 compatibility views.
9. Freeze the native-state schema/capability version, migration, compatibility-
   floor, downgrade, rollback-read-only, repair, and unresolved-journal rules.
10. Execute the real repository reducer and existing applied-commit ledger for
    all five effects, proving it can co-commit preallocated
    commit/member identities and the effect fingerprint frozen only after
    complete validated input for each operation, preserve the prior scope on
    add, atomically fall back from an archived active member, replay exactly,
    and reject commit-ID reuse with a different fingerprint.

Exit gate:

- schemas validate closed fixtures;
- no proposal fixture contains commit authority;
- no read or profile collection crosses a missing/revoked disclosure grant;
- local household approval schemas and transitions are closed and separately
  versioned from backend approval protocol v1;
- cancel/commit races are linearizable and crash-injection fixtures reconcile
  from the co-committed applied-commit ledger;
- commit evidence is securely rederived after a genuine native-repository
  close/reopen and finalized household encryption-key rotation; a durable
  repository reservation must predate dispatch, authoritative absence must
  atomically fence the exact commit before issuing proof, and a proposal-layer
  verifier plus synthesized state cannot replace repository authority;
- raw proposal references never enter native evidence records; pre-dispatch
  terminal cleanup, applied-ledger compaction, 30-day orphan/deny pruning,
  capacity recovery, and maximum-cardinality broker size all pass;
- local-intake completion, digest/fingerprint freeze, generation advance, and
  transition to review are atomic across crash/cancel/revocation races;
- the TUI grammar, attached-human checklist, and renderer rules are frozen;
- native-state migration and downgrade fixtures validate closed declarations;
- the crate graph remains acyclic;
- every operation has an explicit consent, cancellation, retry, and recovery
  classification; and
- Rust, security/privacy, CLI/TUI, and agent-integration specialist reviews
  return GO on one exact SHA.

### Phase 1 — Typed read-only household surface

**Purpose:** give agents truthful household understanding before mutations.

Deliverables:

1. Extract or reuse application controllers for account-bound household reads.
2. Add the two one-shot JSON read commands and two MCP read tools.
3. Implement request-scoped subject resolution without persistent scope
   mutation.
4. Add pagination, output budgets, stable ordering, duplicate-name rejection,
   and privacy minimization.
5. Add manifest-v3 capability, command, schema, authority, disclosure, and tool
   rows while retaining frozen explicit v1/v2 views.
6. Update the embedded skill and OpenClaw candidate to use exact discovery and
   truthful TUI handoff for unsupported actions.
7. Add the binary-owned compatibility/remediation command and its closed result
   schema without widening schema v2 in place.
8. Produce an exact signed private Phase 1 candidate and deliver its executable,
   manifest, schemas, command inventory, and MCP tool list to the OpenClaw skill
   owner for binary-derived shared-skill work.

Exit gate:

- self, exact member, Everyone, omitted active scope, duplicate name, archived,
  incomplete, stale, corrupt, and cross-account cases pass;
- adult/minor/unknown-age, missing/partial/revoked disclosure, Everyone with one
  ungranted member, Everyone with an adapter-omitted member and decremented raw
  count, same-OS-user local-caller, different-OS-user denial, and
  concurrent-revocation cases match the declared boundary;
- output contains no forbidden profile or platform data;
- CLI and MCP results are semantically identical;
- the original six MCP tools remain behaviorally unchanged;
- no household mutation port is reachable from the agent adapter;
- one exact skill artifact first passes the full public `v0.7.0`
  absent-capability and truthful-TUI-handoff matrix as a primary case, then
  produces the correct discovered read behavior against the exact signed Phase
  1 candidate;
- the shared skill contains no residual unconditional household-support or
  household-non-support claim; and
- installed Codex, Claude Code, and OpenClaw discovery tests pass.

Phase 1 may ship privately for review but does not activate mutation claims.

### Phase 2 — Native lifecycle completion

**Purpose:** implement the underlying human/application capabilities once.

Deliverables:

1. Add typed application use cases for edit, archive, restore, and persistent
   scope changes.
2. Preserve human `/household add` atomic member/profile/selected-scope
   semantics with an explicit final scope transition; agent-prepared Add
   changes scope only through an explicit bundled sub-operation.
3. Add revisioned patch validation and safety-critical clearing semantics.
4. Add TUI review and direct-management flows for every enabled operation,
   including grant/revoke controls for per-subject agent access.
5. Implement archive-related scope transition and restoration behavior.
6. Reuse the applied-commit ledger for preallocated identities, durable
   receipts, and crash/interruption reconciliation at the repository boundary.
7. Keep permanent erasure absent and truthfully route removal to archive.
8. Install and verify the new native-state floor before the first new-format
   write; add migration, downgrade-refusal, repair, and rollback-read-only
   behavior.

Exit gate:

- human attached-terminal add/edit/archive/restore/scope journeys pass;
- cancellation before dispatch produces no revision;
- dispatch interruption reconciles the original operation exactly once;
- account rollover, logout, repair, downgrade, and concurrent mutation cases
  fail closed;
- existing `v0.7.0` add/onboard/scope semantics do not regress; and
- direct flows and agent-prepared flows render identical exact changes through
  the frozen TUI grammar and safe-renderer contract; and
- exact-SHA Rust, TUI, persistence, privacy, and security reviews return GO.

### Phase 3 — Agent proposal lifecycle

**Purpose:** let agents prepare useful exact changes without commit authority.

Deliverables:

1. Add MCP prepare, get-status, and cancel tools for enabled operations.
2. Persist encrypted pending proposals with hidden frozen bindings and bounded
   expiry.
3. Add the TUI pending-change inbox and exact review document.
4. Prove that model-visible proposal data cannot authorize or reconstruct a
   commit.
5. Add cancellation, expiry, staleness, conflict, account replacement, and
   lifecycle-generation handling.
6. Add host-independent bare-heyfood handoff instructions to the canonical
   skill.
7. Add terminal-state payload purge, replay tombstones, disclosure-revocation,
   and logout/account-replacement cleanup.
8. Add disclosure-filtered presentation/status builders and fixtures for
   prepare-to-local-input-to-status, mid-flight revocation, Everyone,
   cancellation, restart, and reconciliation.

Exit gate:

- prepare/status/cancel are useful on every supported agent host;
- prepare proves zero household mutation; cancellation accepted before dispatch
  proves zero mutation;
- changed repository or profile state invalidates the proposal;
- hostile labels/profile values cannot alter review instructions;
- agent PTY/TUI automation remains rejected; and
- privacy-safe installed-artifact evidence passes on macOS and Linux.

### Phase 4 — Local trusted approval and exact-once commit

**Purpose:** allow authorized agent-orchestrated mutations without weakening
human control.

Deliverables:

1. Implement the separately versioned local household review broker and exact
   proposal-digest binding.
2. Implement attached-TUI `Save changes`/`Cancel` controls and retain only the
   agent prepare, status, pre-dispatch cancel, and reconciliation tools.
3. Bind local review to account, disclosure generation, household revision,
   member/profile revision, operation, exact diff, expiry, MCP session, and
   lifecycle generation.
4. Commit once under the lifecycle lock using the co-committed applied-commit
   marker, then derive and verify the resulting receipt.
5. Add cancellation linearization, uncertain-outcome blocking, and
   reconciliation before any later change.
6. Keep agent confirmation, hosted approval, and permanent erasure absent.

Exit gate:

- approved add/edit/archive/restore/scope changes commit exactly once;
- cancel, reject, expiry, stale state, cross-account/session, altered diff,
  model approval, agent-host approval, PTY automation, replay, and unsupported
  host produce no mutation;
- cancellation that loses the `committing` CAS returns too-late status and
  reconciles instead of claiming cancellation;
- every uncertain case reconciles before later mutation;
- permanent erasure and an agent confirm tool remain absent;
- real Codex, Claude Code, and OpenClaw host/version matrices pass; and
- exact-SHA Rust, security/privacy, agent-behavior, and release reviews return
  GO.

Phase 4 source may merge behind a closed capability gate, but no public skill,
manifest, or MCP server advertises agent confirmation or permanent erasure.

### Phase 5 — Installed-artifact qualification and `v0.8.0` rollout

**Purpose:** prove the experience from exact signed public-candidate bytes.

Minimum clean-agent journeys:

| Journey | Required evidence |
|---|---|
| Cold discovery | Default qualified manifest, household capability, exact tool/command inventory |
| Schema compatibility | Explicit v1/v2 remain valid and omit v3 household claims; unknown schemas fail closed |
| Public `v0.7.0` + installed 1.0.2 | Existing human-TUI handoff remains truthful |
| Shared skill on public `v0.7.0` — primary compatibility case | No household agent claim; truthful human-TUI handoff |
| `v0.8.0` + stale 1.0.2 | Version-invariant bootstrap returns the exact update remedy and no household agent action |
| Shared skill on `v0.8.0` | Uses only manifest/tool-discovered household operations |
| Pinned/unpinned OpenClaw installs | Pinned behavior stays explicit; unpinned cold resolution obtains the reviewed digest |
| Missing/unknown skill identity | Binary-owned fail-closed instruction is available without trusting model-supplied metadata |
| Household context read | Correct roster/scope/readiness with no private diagnostics |
| Member profile read | Exact stable reference and minimized declared projection |
| Disclosure boundary | Ungranted/revoked/minor/unknown-age data remains absent; all same-OS-user callers receive only the expressly granted projection; Everyone never degrades to a partial household |
| Request-scoped targeting | Correct self/member/Everyone result without persistent scope change |
| Add preparation | Preallocated identity, local sensitive intake, explicit scope transition, and zero mutation before TUI save |
| Edit preparation | Exact safety-critical before/after diff |
| Archive/restore | Correct recoverability and scope consequences |
| Scope change | Conversation continuity reset and persisted exact scope |
| Local approval | Human reviews the exact proposal in the attached TUI with no profile upload |
| Local commit | One revision and applied-commit marker only after valid TUI `Save changes` |
| Cancel/expiry/reject | Proven non-mutation |
| Cancel/commit race | Exactly one CAS winner; too-late cancellation reconciles truthfully |
| Concurrent/stale state | Typed rejection and fresh preparation requirement |
| Uncertain outcome | Reconciliation before any retry or later mutation |
| Erasure | Operation, command, skill claim, and tool are absent |
| Cross-account/logout | No content or proposal crosses the account lifecycle |
| Hostile content | Labels/profile text remain data and cannot change policy |
| Native-state upgrade | Public v0.7 state migrates atomically; interrupted migration resumes; managed downgrade refuses before replacement |
| Human parity | Direct TUI management and pending-change inbox remain complete without an agent host |

Qualification requirements:

- latest supported Codex and Claude Code local hosts;
- the coordinated OpenClaw skill candidate;
- the public `v0.7.0` shared-skill path runs first and receives the complete
  positive handoff, absent-tool, unsupported-operation, and fail-closed matrix,
  not a reduced regression subset;
- macOS Apple Silicon and Intel signed/notarized archives;
- Linux ARM64 and x64 attested archives;
- exact skill, manifest, schema, MCP, executable, installer, and archive digests;
- macOS and Linux source CI for the four supported distribution targets;
- an updated `HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md` pass on all four exact
  artifacts at compact/standard/wide widths, with `NO_COLOR`, Esc/Ctrl+C,
  restart/resume, normal/signal/failure terminal restoration, every direct
  lifecycle operation, every inbox state, and clear saving-versus-saved copy;
- attached-human evidence records only content-free PASS/FAIL categories—no
  screenshots, terminal captures, transcripts, proposal/member references,
  profile values, account identifiers, paths, or approval content;
  and
- production-like canaries using synthetic household data only.

Rollout order:

1. align Cargo/workspace version, installer `SUPPORTED_VERSION`,
   `NATIVE_STATE_RELEASE_VERSION`, changelog, signing/release documents, and
   native-state declaration at `0.8.0`;
2. freeze the exact SHA intended to become both `main` and the annotated tag,
   then run protected qualification with `qualify_signed_candidate=true` at
   that SHA;
3. bind the protected run ID, its unexpired ten-file aggregate,
   `SHA256SUMS` digest, manifest/schema/skill digests, and approval variables;
   no asset may be rebuilt, resigned, repackaged, or regenerated afterward;
4. obtain exact-SHA specialist GO and candidate exact-byte GO, then privately
   provide those exact candidate bytes to the OpenClaw skill owner;
5. qualify one shared skill artifact against public `v0.7.0` first and the
   private exact `v0.8.0` candidate second;
6. publish the skill and begin active propagation observation;
7. prove the intended version resolves as ClawHub `latest` from two clean
   resolutions, bind its digest, and rerun the exact candidate with that skill;
8. merge only with exact-SHA preservation. If main, tag, product, manifest,
   schema, MCP, embedded-skill, or compatibility bytes differ, discard the
   approval bindings and restart protected candidate and skill qualification;
9. create the annotated `v0.8.0` tag at the exact reviewed main/protected-run
   SHA and publish only the approved aggregate bytes;
10. smoke all public assets on macOS Apple Silicon/Intel and Linux ARM64/x64
    with the reviewed checked-in installer;
11. deploy the exact reviewed installer and checksum to `hey.food`, then run
    explicit hosted-installer verification on all four targets and verify
    served-byte identity plus default `v0.8.0` installation;
12. activate public documentation/support claims only after hosted smoke, then
    verify cold-agent discovery and one approved synthetic local-TUI change
    from public bytes; and
13. monitor content-free mutation, cancellation, uncertainty, reconciliation,
    and migration categories, then clear stale protected bindings after
    successful completion or abandonment.

If hosted cutover or post-cutover smoke fails, keep the prior hosted installer
and support copy active, fail closed on default installation of the candidate,
and fix forward through a new reviewed installer digest. Do not rewrite the
published release assets.

## Test and evidence matrix

### Rust and contract gates

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

Additional required suites:

- manifest v1/v2 frozen compatibility, v3 default, unknown-schema rejection,
  and version-invariant bootstrap tests;
- Clap/help/completion/manifest inventory parity;
- MCP initialization, exact tool list, schemas, annotations, cancellation,
  concurrency, framing, size, pagination, and slow-reader tests;
- CLI/MCP/TUI application-controller parity;
- encrypted repository and lifecycle-lock tests;
- proposal serialization, redaction, expiry, replay, and authority-separation
  tests;
- add/edit/archive/restore/scope property and state-machine tests plus proof
  that erasure and agent confirmation are absent;
- account replacement, logout, repair, migration, and downgrade-floor tests;
- crash points before, during, and after repository publication;
- TUI terminal restoration and attached-human local review/commit tests; and
- installed Agent Skill discovery and behavior tests for every supported host.

### Release-evidence allowlist

Archived evidence may contain only:

- exact product SHA/tree and protected run ID;
- archive, checksum, manifest, schema, executable, installer, and skill digests;
- target triple plus agent host/version;
- closed content-free PASS/FAIL or approved failure-category values; and
- synthetic-canary correlation identifiers that cannot resolve to account or
  household content outside the restricted test system.

Screenshots, recordings, terminal captures, transcripts, display labels,
stable member/proposal references, profile fields, account identifiers,
repository paths, local approval content, cookies, tokens, and credentials are
forbidden even when the fixture is synthetic.

### Negative security gates

The following must remain zero:

- mutation from natural language alone;
- mutation from a model-visible token or proposal reference;
- mutation from a generic host approval dialog;
- cross-account or cross-member confusion;
- first-match duplicate-name resolution;
- safety-field clearing by omission;
- approval replay or approval applied to a changed proposal;
- automatic retry after uncertain dispatch;
- agent-driven TUI/PTY interaction;
- raw profile data in logs, errors, traces, evidence, or panic output;
- arbitrary filesystem, shell, URL, credential, or repository access; and
- public claims for an absent or unqualified tool.

## Documentation and integration deliverables

Implementation updates these surfaces together:

```text
AGENTS.md
CLAUDE.md
README.md
docs/AGENT_INTEGRATION.md
docs/AGENT_SAFETY.md
docs/AGENT_APPROVAL_CONTRACT.md
docs/LOCAL_HOUSEHOLD_APPROVAL_CONTRACT.md
docs/HOUSEHOLD_LOCAL_STATE.md
docs/HOUSEHOLD_TUI_MANUAL_ACCEPTANCE.md
docs/NATIVE_STATE_COMPATIBILITY.md
docs/RELEASE_SIGNING.md
docs/CLI_CONTRACT.md
docs/COMMAND_GRAMMAR.md
docs/CAPABILITY_STATUS.md
docs/JSON_SCHEMAS.md
schemas/v2/heyfood-agent-manifest.schema.json
schemas/v3/heyfood-agent-manifest.schema.json
agent-integrations/skills/heyfood/SKILL.md
install.sh
.github/workflows/release.yml
.github/workflows/post-release-smoke.yml
```

The compatibility command and its result schema are also documented here. The
OpenClaw skill contains no unconditional household support sentence and no
hardcoded upgrade path. Its household routing is generated from or tested
against exact manifest/tool fixtures for every supported binary generation.
Its frontmatter and package metadata contain the exact skill contract version.

The public hey.food landing page should mention agent-aware household context
only after installed-artifact qualification. Copy must remain approachable and
specific: agents can understand and prepare household changes; heyfood keeps
the final decision with the person. It must not imply remote household sync,
health integration, unattended mutation, or TUI automation.

## Review requirements

Each phase returns one exact product SHA and privacy-safe evidence digest.
Specialist subagent review is required because an implementation author cannot
provide the sole approval for its own phase.

| Review | Required focus |
|---|---|
| Lead Rust specialist | crate direction, state machines, deterministic schemas, cancellation, concurrency |
| Security/privacy specialist | account binding, disclosure consent, proposal authority, replay, deferred-erasure claims, redaction |
| CLI/TUI contract specialist | human parity, approval UX, JSON streams, help/completion, terminal restoration |
| Agent/OpenClaw specialist | cold discovery, skill compatibility, MCP schemas, safe orchestration |
| Release specialist | exact artifacts, signatures, attestations, installer, capability and website claims |

No review may approve a later phase merely because its code is present. An
unresolved finding remains in the phase inventory until independently closed
at a new exact SHA.

## Program sequencing and parallelism

```text
Phase 0 contract and threat model
                 |
                 v
       Phase 1 read-only surface
                 |
                 v
     Phase 2 lifecycle completion
                 |
                 v
      Phase 3 proposal lifecycle
                 |
                 v
   Phase 4 trusted approval/commit
                 |
                 v
  Phase 5 installed release qualification
```

After Phase 0 freezes the schemas, these bounded workstreams may proceed in
parallel:

- application/repository lifecycle implementation;
- manifest-v3 and read-only MCP/CLI adapters;
- TUI direct-management and pending-review UX;
- local review broker, disclosure, and threat-model fixtures; and
- OpenClaw/Codex/Claude skill compatibility work.

They reconverge before Phase 3. No adapter invents a private lifecycle rule to
move ahead of the shared application contract.

## Effort estimate

The expected cumulative effort is approximately 25–40 engineer-days:

| Workstream | Estimate |
|---|---:|
| Contract, threat model, schema, executable proof | 3–5 days |
| Read-only CLI/MCP household surface | 3–5 days |
| Edit/archive/restore/scope lifecycle and TUI parity | 6–9 days |
| Proposal persistence, disclosure, review inbox, local broker | 6–10 days |
| Local commit/reconcile, migration, and retention gates | 3–6 days |
| Skill integration, installed-artifact qualification, release | 4–5 days |

This estimate is credible only for the attached-TUI approval path, reuse of the
existing applied-commit ledger, and permanent erasure remaining absent. A
hosted/out-of-band approval system or permanent-erasure program requires a new
estimate after its own Phase 0. Parallel work before the shared contract closes
is non-shipping prototype work and may not activate a capability.

## Release decision rules

`v0.8.0` may ship household reads without lifecycle preparation only if public
capability copy says so exactly. It may ship agent preparation for
add/edit/archive/restore/scope only with the attached-TUI approval/commit path.
Permanent erasure, hosted approval, and agent confirmation remain absent from
the release and all public claims.

The release is HOLD if any of the following remains:

- manifest/skill/tool disagreement;
- missing exact-scope or account binding;
- missing or revoked per-subject disclosure authorization;
- model-visible commit authority;
- unreviewed safety-critical profile clearing;
- missing cancellation or uncertainty reconciliation;
- incomplete direct-human TUI parity for an advertised operation;
- failed signed installed-artifact or public-download test;
- household content in evidence or diagnostics; or
- public wording broader than the qualified surface.

## Definition of done

The program is complete when:

- a cold supported agent discovers household capabilities from the installed
  binary without repository access;
- the agent can read the correct minimized local context for self, one member,
  or Everyone;
- the agent can prepare every advertised lifecycle change without mutation;
- the attached heyfood TUI—not the agent host—obtains the person's exact
  approval and invokes the local commit;
- approved changes commit exactly once against frozen account and revision
  authority;
- cancellation, rejection, stale state, replay, hostile content, unsupported
  hosts, and ambiguous consent do not mutate;
- uncertain outcomes reconcile before retry;
- human TUI household management remains excellent and independent of agent
  tooling;
- the OpenClaw, Codex, and Claude skills agree with the exact manifest and MCP
  tool list;
- signed public artifacts, installer, schemas, skills, and documentation are
  bound to reviewed bytes; and
- all exact-SHA, exact-byte, security/privacy, agent-behavior, and release
  reviews return GO.
