# heyfood agent approval contract

**Status:** Phase 0 protocol v1 freeze; not implemented or advertised

This contract applies only to protected mutations initiated through a
qualified MCP integration. Agent-safe mutating one-shot CLI commands are
outside the current program. The normative presentation and internal wire
envelopes are:

- `schemas/v1/agent-proposal-presentation.schema.json`
- `schemas/v1/agent-approval-protocol.schema.json`

Any incompatible field or state transition requires a new protocol version.

## Identity and session binding

The backend creates every approval session after normal account
authentication. It generates:

- a random UUID `approval_session_id`;
- a uniformly random 256-bit `session_binding_token`; and
- a bounded expiry no longer than 15 minutes.

The binding token is returned only to the local MCP process, retained only in
locked process memory, transmitted only as an authenticated backend header,
and never serialized into a tool result, model context, log, host
configuration, setup receipt, URL, evidence, or persistent local state.
`clientInfo`, process ID, parent metadata, host labels, tool arguments, and
other host/model-supplied values are diagnostic only and never establish
identity or authority. The backend account subject plus its own session ID and
binding-token hash are authoritative.

Restart, EOF, logout, account rotation, expiry, or binding-token loss creates
a new approval session. No approval crosses that boundary.

## Prepare result

The agent receives the allowlisted `AgentProposalPresentation` v1 containing
only:

- schema version and mutation family;
- stable non-secret resource identifiers;
- exact human display fields, including safety, intended-for, substitutions,
  label guidance, freshness, and provenance where applicable;
- proposal digest;
- frozen precondition summaries; and
- a non-capability approval reference.

The presentation is constructed field-by-field from the schema allowlist. It
must never contain a production confirmation token, idempotency authority,
backend commit credential, bearer credential, or serialized production
proposal wire. Serialization starts from this allowlist; it does not serialize
then redact a production DTO.

`proposal_digest_sha256` is SHA-256 over the UTF-8 bytes of the RFC 8785 JSON
Canonicalization Scheme representation of the server-held immutable proposal
record excluding `proposal_digest_sha256`, approval/session references, and
all transport credentials. That record includes the mutation family,
operation, account subject, resource references, complete human display
fields, frozen preconditions, and expiry. The backend computes and verifies
the digest; the model cannot supply or replace the hashed record.

`approval_reference` is a server-generated random 256-bit base64url lookup
reference. It is not a capability: requests still require the matching
authenticated account and approval session, and possession alone cannot
approve, cancel, consume, or commit.

## Backend records and endpoints

All endpoints are fixed to the production hello.food origin and require the
normal account-bound session. MCP-only endpoints additionally require the
session binding token.

| Method and path | Caller | Effect |
|---|---|---|
| `POST /v1/agent-approval/sessions` | local MCP | Create a backend-generated approval session |
| `POST /v1/agent-approval/proposals` | bound MCP | Store one immutable server proposal and return its allowlisted presentation plus HTTPS approval URL |
| `GET /v1/agent-approval/proposals/{approval_reference}` | authenticated human page | Render the exact stored proposal for the matching account |
| `POST /v1/agent-approval/proposals/{approval_reference}/decision` | authenticated human page | Record one accept or decline using compare-and-swap |
| `GET /v1/agent-approval/approvals/{approval_id}` | bound MCP | Observe status or reconcile an uncertain operation |
| `POST /v1/agent-approval/approvals/{approval_id}/commit` | bound MCP | Atomically consume one approval and accept one backend commit |
| `POST /v1/agent-approval/approvals/{approval_id}/cancel` | bound MCP | Cancel a still-pending approval without product mutation |

The approval URL is `https://auth.hello.food/agent-approval/{approval_reference}`.
It contains no account identifier, proposal data, digest, decision, session
identity, binding token, commit token, bearer credential, or redirect target.
Query parameters and alternate origins are rejected.

## State machine

The only legal transitions are:

```text
prepared -> awaiting_human
prepared | awaiting_human -> cancelled | expired | invalidated
awaiting_human -> approved | declined
approved -> committing
committing -> committed | reconciliation_required
reconciliation_required -> committed | invalidated
```

Every transition is a database compare-and-swap over approval ID, account,
approval-session ID, proposal digest, state, expiry, and unused generation.
Declined, cancelled, expired, invalidated, or committed records are terminal.
The decision endpoint is idempotent only for an identical already-recorded
decision; a conflicting replay fails closed.

The `approved -> committing` consume and creation of the backend
mutation/transaction record occur in one serializable database transaction
with a unique constraint on `approval_id`. A second consume cannot create a
second commit. Product mutation runs under that unique transaction/outbox
identity. If commit observation is lost after acceptance, the state becomes
`reconciliation_required`; MCP observes approval and resource state and never
blindly submits a second commit.

## Approval sequence

1. MCP creates a backend approval session and prepares or validates the exact
   server proposal.
2. The backend freezes the proposal, digest, preconditions, account, session,
   expiry, and single-use identity.
3. On an eligible exact host/version, MCP starts URL-mode elicitation for an
   `https://auth.hello.food/...` URL containing no credential, personal data,
   pre-authenticated capability, or commit token.
4. The account owner authenticates independently on hello.food.
5. The page renders the exact frozen proposal and records accept or decline.
6. The originating bound MCP session observes the record and commits at most
   once through the atomic consume transition.
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
- host `clientInfo`, executable PID, parent metadata, forged session IDs,
  approval references without the binding token, and binding tokens from a
  different account/session do not authorize;
- no current or future capability field enters `AgentProposalPresentation`;
- stale, replayed, cross-account, cross-session, expired, cancelled, or
  modified proposals do not mutate;
- concurrent identical commits create one backend mutation, while conflicting
  decisions and commits fail closed;
- future production proposal fields cannot enter the presentation unless the
  presentation schema version and builder allowlist are explicitly revised;
- MCP absence never falls back to a human mutation command;
- missing controlling terminal prevents direct human CLI dispatch; and
- uncertain commit observation returns reconciliation guidance without retry.
