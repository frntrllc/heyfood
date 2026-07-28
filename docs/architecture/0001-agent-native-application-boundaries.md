# ADR 0001: agent-native application boundaries

**Status:** Accepted for Phase 0 implementation proof  
**Date:** 2026-07-27  
**Baseline:** `d68091a9cf6341c2c9120ba9251a6e0dd79a9616`

## Context

heyfood now has a real Rust TUI and one-shot CLI. The workspace already has a
sound directional split:

```text
core <- application <- runtime/platform
  ^          ^              ^
  |          |              |
 cli/tui ----+---------- bin composition
```

`RunTurn`, `EnsureSession`, `SerializedStateWriter`, the event stream, and
several ports live in `heyfood-application`. However, the concrete
`OneShotExecutor` and interactive panel driver in `heyfood-bin` still own
Grocery, Menu Watch, capability/status, household-context, and presentation
orchestration while calling `HttpService` directly. `GroceryPort` exists but
has no production adapter or application controller. There is no application
Menu Watch port or capability/status controller.

Putting MCP handlers beside the current binary handlers would create a second
client implementation and eventually drift from the TUI/CLI.

## Decision

1. `heyfood-bin` remains the only executable and the composition root.
2. Agent work first extracts renderer-neutral application controllers and
   object-safe ports for:
   - conversational turns;
   - capability/status discovery;
   - Grocery read, prepare, cancel, and human confirmation;
   - Menu Watch read and human management.
3. The current CLI/TUI must route through each controller with fixture parity
   before MCP is allowed to use it.
4. HTTP DTO conversion stays in `heyfood-agent-runtime`; terminal or JSON
   rendering stays in `heyfood-cli`/`heyfood-tui`.
5. Agent-visible proposal presentation is a separate allowlisted application
   type. It is never a serialized or redacted
   `GroceryMutationProposalWire`.
6. Phase 0 may add internal proof modules and tests, but no public `agent` or
   `mcp` command, installer behavior, plugin, or capability claim.

## Crate decision

The final crate split is intentionally evidence-driven:

- deterministic manifest, guide, and schema types may become
  `heyfood-agent-contract`;
- stdio protocol framing may become `heyfood-mcp`;
- host-specific reversible setup may become `heyfood-agent-setup`.

They remain separate only if the Phase 0 dependency proof shows distinct
dependency, feature, or platform requirements. A single all-purpose agent
crate is rejected because it would couple network-free self-description to
MCP framing and host configuration writes.

No new crate may depend on `heyfood-bin`, `heyfood-cli`, or `heyfood-tui`.
`heyfood-bin` composes inward-facing application contracts with outward-facing
runtime/platform adapters.

## Thin proof

The Phase 0 proof must:

1. load a deterministic internal manifest fixture;
2. construct an application controller through an object-safe fake port;
3. perform one account-bound, cancellable read;
4. prove cancellation reaches the port;
5. compile from a `heyfood-bin` integration test; and
6. remain unreachable from the shipped Clap tree.

The proof establishes dependency direction, not public agent functionality.

## Consequences

- Some code moves out of the 4,000-line binary library before agent features
  are added.
- CLI/TUI parity tests become mandatory during extraction.
- MCP handlers remain thin and renderer-neutral.
- Runtime adapters can be tested independently from terminal state.
- Agent phases can ship self-description before any MCP mutation exists.

## Rejected alternatives

- **Teach agents to drive the TUI:** terminal state and human presentation are
  not stable machine contracts.
- **Wrap `heyfood` subprocesses from MCP:** this loses typed cancellation,
  authority, and error semantics and enables unsafe mutation fallback.
- **Let MCP call `HttpService` directly:** this duplicates application rules.
- **Install `AGENTS.md` beside the executable:** agent hosts do not discover
  arbitrary instruction files on `PATH`.
