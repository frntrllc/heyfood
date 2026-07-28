# heyfood agent approval contract

**Status:** Phase 0 protocol draft; not implemented or advertised

This contract applies only to protected mutations initiated through a
qualified MCP integration. Agent-safe mutating one-shot CLI commands are
outside the current program.

## Prepare result

The agent receives an allowlisted `AgentProposalPresentation` containing only:

- schema version and mutation family;
- stable non-secret resource identifiers;
- exact human display fields, including safety, intended-for, substitutions,
  label guidance, freshness, and provenance where applicable;
- proposal digest;
- frozen precondition summaries; and
- a non-capability approval reference.

It must never contain a production confirmation token, idempotency authority,
backend commit credential, bearer credential, or serialized production
proposal wire. Serialization starts from this allowlist; it does not serialize
then redact a production DTO.

## Approval sequence

1. MCP prepares or validates the exact server proposal.
2. On an eligible exact host/version, MCP starts URL-mode elicitation for an
   `https://auth.hello.food/...` URL containing no credential, personal data,
   pre-authenticated capability, or commit token.
3. The account owner authenticates independently on hello.food.
4. The page renders the exact proposal and records accept or cancel.
5. The server binds the record to account/subject, MCP client and server
   session nonces, proposal digest, operation, frozen list/version/context
   preconditions, expiry, and single use.
6. The originating MCP operation observes the record and commits at most once.
7. The client verifies resulting state. An uncertain outcome is reconciled,
   never blindly retried.

Decline, expiry, disconnect, cancellation, process restart, changed proposal,
changed context, different account/session, or missing host elicitation
capability leaves the proposal uncommitted.

## Human CLI compatibility

The legacy human Grocery workflow may continue to transport its complete
proposal on stdin and its result on contracted stdout. Before dispatch, it
must render the exact proposal and collect a fresh accept/cancel decision from
a separate attached controlling terminal. Proposal bytes and
`--decision accept` alone are insufficient.

Meal logging and Menu Watch mutation commands receive equivalent
per-command human-terminal transport rows before agent setup can ship.

## Required negative tests

- model prose, tool arguments, stdin, form-mode elicitation, and ordinary host
  permission prompts do not authorize;
- no current or future capability field enters `AgentProposalPresentation`;
- stale, replayed, cross-account, cross-session, expired, cancelled, or
  modified proposals do not mutate;
- MCP absence never falls back to a human mutation command;
- missing controlling terminal prevents direct human CLI dispatch; and
- uncertain commit observation returns reconciliation guidance without retry.
