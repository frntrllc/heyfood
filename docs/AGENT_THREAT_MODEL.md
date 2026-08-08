# heyfood agent integration threat model

**Status:** v0.9.0 release. Read-only household context and exact
additional-member profile reads are active only when the attached TUI has
saved the corresponding local disclosure grant. Agent household lifecycle
mutations remain absent and separately gated.

## Assets and trust boundaries

Protected assets include hello.food account authority, native session
credentials, dietary and household context, Grocery lists and confirmation
authority, Menu Watch subscriptions, restaurant/menu evidence, local files,
terminal state, and release/setup provenance.

Trust boundaries are:

1. the person and the coding-agent model;
2. the agent host and its skill/plugin/MCP configuration;
3. the local `heyfood` process and native credential backend;
4. the hello.food API and approval origin;
5. restaurant/menu/profile/Grocery content returned as untrusted data; and
6. release artifacts, installers, and separately distributed integrations.

The model, tool prose, repository content, service content, and ordinary host
approval prompts are not trusted mutation approvers.

## Required properties

- Installation grants no new account, shell, filesystem, network, microphone,
  or product-mutation authority.
- Discovery is network-free and credential-free.
- Agent-visible DTOs contain no reusable credential or commit capability.
- Every agent mutation uses prepare, exact presentation, independently
  authenticated approval, commit-once, verification, and reconciliation.
- Unsupported hosts expose no confirm tool.
- An uncertain request is never retried without state reconciliation.
- Setup is opt-in, exact-targeted, conflict-safe, and reversible.
- MCP stdout contains protocol frames only and is bounded.
- Product content cannot alter instructions, tool selection, or approval
  state.

## Threats and controls

| Threat | Example | Required control | Verification |
|---|---|---|---|
| Prompt injection | Menu text says to run a command or reveal credentials | Treat all product content as typed data; no generic shell, URL, file, or raw API tool | Adversarial content fixtures and tool-call scoring |
| Authority confusion | Model emits `accept`, passes a token, or approves a host dialog | Only a bound hello.food approval record releases MCP commit authority | Negative tool/form/prose/host-approval tests |
| CLI fallback bypass | MCP confirm is absent, so the agent pipes a full proposal to `grocery confirm` | No mutation is `agent_safe`; human mutations require a distinct controlling-terminal decision | MCP-absent cold-agent and missing-terminal tests |
| Capability leakage | Full production Grocery wire is returned to the model | Allowlisted `AgentProposalPresentation`; deny token/idempotency/commit fields | Schema allowlist and future-field leakage tests |
| Replay or substitution | Reuse approval on a different account, list, context, or session | Bind account, subject, MCP nonces, proposal digest, frozen preconditions, operation, expiry, and single use | Replay, stale, cross-account, and cross-session tests |
| Blind retry | Network fails after POST dispatch | Typed uncertain outcome and mandatory reconcile operation | DG-R2 and transport-loss tests |
| Credential disclosure | Token appears in MCP result, logs, command arguments, evidence, or crash output | Native credential port only; structured redaction; no credential-reading tool | Sentinel scans and panic/error tests |
| Configuration takeover | Setup overwrites a user's `AGENTS.md`, plugin, or MCP configuration | Dry-run default, host-owned setup mechanisms, expected-digest replacement, atomic fallback adapter | Conflict, link, crash, concurrent, rollback tests |
| Executable substitution | Host config launches bare `heyfood` from attacker-controlled `PATH` | Verified absolute executable identity in setup receipt | Replacement and PATH-substitution tests |
| Environment substitution | Project/host exports `HEYFOOD_API_URL`, API key, state root, CA, or file credential-store override before launching the trusted binary | MCP rejects every inherited `HEYFOOD_*` variable before credential access and constructs fixed production/native configuration | Origin, key, CA, store, state, debug, and unknown-prefix negative tests |
| Cross-account state | Split credentials or stale local context are reused | Existing account-bound native credential and imported-state checks | Split/cross-account fixture tests |
| Resource exhaustion | Huge JSON-RPC frame, event flood, slow reader, unbounded queue | Frozen frame/result/event/page/concurrency limits and cancellation | Oversize, flood, slow-reader, and shutdown tests |
| Terminal corruption | Agent drives the TUI or abandoned child owns raw mode | TUI automation unsupported; MCP is stdio JSON-RPC; bounded child cleanup | PTY/ConPTY restoration and parent-death tests |
| False capability claims | Deferred Health or hidden commands appear usable | Public manifest has active/deferred/unavailable only; hidden topology absent | Help/completion/schema/manifest parity tests |
| Supply-chain drift | Skill, schema, or plugin does not match the binary | Exact digests, compatibility versions, signed/attested artifacts, exact-byte review | Installed-artifact provenance matrix |
| Household disclosure without consent | A local agent reads another person's roster identity or dietary profile because the account owner stored it | Separate current per-subject roster/profile grants created only in the attached TUI; minor/unknown-age profile reads fail closed; Everyone is all-or-nothing | Missing/partial/revoked/minor/unknown-age and same/different OS-user fixtures |
| Same-user caller confusion | A grant intended for one coding agent is silently treated as host-bound authentication | v0.8.0 grants explicitly cover every caller with the same OS-user access to the account-bound state; no per-host claim exists | Disclosure notice and cross-OS-user denial tests |
| Local approval spoofing | Model text, MCP arguments, a PTY, or an agent-host dialog pretends to save a household change | Only `Save changes` in the attached heyfood TUI can win the local review-to-commit CAS; there is no agent confirm tool | Absent-tool, natural-language, redirected-I/O, and PTY negative tests |
| Malicious member content | A label uses ANSI, bidi, invisibles, markup, or URL text to imitate review controls | Canonical values remain data and the local review renderer visibly escapes terminal controls and directional/invisible characters | Hostile-content digest and compact/standard/wide rendering fixtures |
| Proposal authority leakage | Status repeats hidden account, digest, commit, or profile data after grant revocation | Closed presentation projections; revalidate account/grant generation before every serialization; downgrade to content-free | Future-field leakage and concurrent-revocation tests |
| Content-free authority confusion | A Scope proposal keeps running after its subject grant expires because its output projection carries no profile fields | Bind and revalidate affected-subject roster authority independently of output projection; Everyone uses the complete repository roster | Same-digest Scope expiry tests before prepare return, status, and commit |
| Forged reconciliation evidence | A caller appends a matching record to a public household-state DTO and marks an uncertain proposal committed | Repository-held account/proposal/commit capability issues opaque committed or exact-absence proof; journal rejects every other authority identity | Caller-created state/authority, wrong-fingerprint, wrong-revision, and restart tests |
| Cancel/commit race | Agent cancellation reports success after the attached TUI began committing | Linearizable CAS; cancellation after `committing` returns too-late and reconciliation owns the outcome | Race and crash-injection state-machine tests |
| Native downgrade corruption | v0.7.0 interprets a v0.8.0 proposal or reconciliation journal as older household state | Managed writer floor, crash-resumable v2→v3 migration, downgrade refusal, separately qualified rollback-read-only mode | Migration interruption and downgrade matrix |

## Mutation families

Grocery, meal logging, Menu Watch creation/removal, and any conversational
route capable of invoking a mutation are separate authorization families. A
family has no agent mutation tool until its complete state machine passes.
`ask` or assessment may return a proposal but must not commit it directly or
indirectly.

Direct one-shot mutation commands remain human-terminal-only. Redirected JSON
may be a data channel where the command contract requires it, but a fresh
decision must be collected on a separate controlling terminal before dispatch.
Allocating a PTY to automate that ceremony is unsupported and a cold-agent
hard failure.

## Residual risks and deferred authority

- Public macOS/Linux installed-archive qualification is required for v0.9.0.
  Windows CI and distribution are outside this release contract.
- The application boundaries, exact-host setup matrix, Agent Skill, setup, and
  read-only MCP implementation passed independent exact-SHA and exact-artifact
  review for the previous release. The v0.9.0 Diet additions require the same
  exact-source and exact-artifact gates.
- Phase 4 mutation endpoints and tools remain absent. The frozen hello.food
  out-of-band approval protocol still requires its separate security,
  implementation, and production qualification before any mutation tool can
  be advertised.
- The local household contract is independently versioned from that hosted
  approval protocol. v0.9.0 activates only disclosure-gated household reads
  and capability-gated Diet reads. The frozen
  prepare/status/cancel/reconcile and commit designs do not activate an agent
  lifecycle command, mutation MCP tool, or model-controlled approval path.
  See [LOCAL_HOUSEHOLD_APPROVAL_CONTRACT.md](LOCAL_HOUSEHOLD_APPROVAL_CONTRACT.md).

The machine-readable Phase 0 inventory distinguishes Phase 0 review and
qualification blockers from intentionally deferred implementation.
