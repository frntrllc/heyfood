# heyfood agent integration threat model

**Status:** Phase 0 implementation checkpoint; independent review pending; no public agent integration is enabled
**Baseline:** `d68091a9cf6341c2c9120ba9251a6e0dd79a9616`

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

## Residual Phase 0 risks

- The public Unix binary PTY test proves proposal stdin remains distinct from
  the controlling-terminal decision. Exact installed-archive qualification
  remains outstanding, including the Windows `CONIN$`/`CONOUT$` path.
  Negative public-binary tests prove missing-terminal rejection occurs before
  credential or network access.
- The frozen hello.food out-of-band approval protocol still requires
  independent security review. Its endpoint implementation and production
  proof are Phase 4 gates, not Phase 0 implementation.
- The application boundaries and exact-host setup matrix are implemented as
  Phase 0 evidence, but still require independent exact-SHA review.
- Agent Skill, setup, and MCP implementation remain deliberately absent until
  their later authorized phases.

The machine-readable Phase 0 inventory distinguishes Phase 0 review and
qualification blockers from intentionally deferred implementation.
