# heyfood agent approval contract

**Status:** Phase 0 protocol v1 freeze; not implemented or advertised

This contract applies only to protected mutations initiated through a
qualified MCP integration. Agent-safe mutating one-shot CLI commands are
outside the current program. The normative presentation and internal wire
envelopes are:

- `schemas/v1/agent-proposal-presentation.schema.json`
- `schemas/v1/agent-approval-protocol.schema.json`
- `fixtures/agent/approval-protocol-v1-lifecycle.json`

Any incompatible field or state transition requires a new protocol version.

The approval schema's top-level `oneOf` is the complete JSON-envelope
registry. Every request and result uses `schema_version: 1`, has a unique
`kind`, rejects unknown fields, and is capped by the MCP frame limits. The
external `AgentProposalPresentation` reference resolves to the exact v1 schema
above. An implementation rejects an unknown kind, field, enum value, schema
version, or unresolved schema reference before credential access or approval
state change.

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

## Transport, headers, browser trust, and CSRF

Bound MCP backend JSON endpoints use only `https://api.hello.food`, TLS, and
`Content-Type: application/json`. Alternate origins, redirects, query
parameters, userinfo, fragments, proxy-derived origin overrides, and
environment-selected origins are rejected. Every MCP call requires the normal
account-bound `Authorization: Bearer <account-bound-session>` header.

`POST /v1/agent-approval/sessions` also requires `Idempotency-Key` equal to
the body's `session_request_id`. Every bound MCP endpoint requires:

```text
X-Heyfood-Agent-Approval-Session: <approval_session_id UUID>
X-Heyfood-Agent-Approval-Binding: <43-character base64url token>
```

The backend compares the authenticated account, its backend session, the
approval-session UUID, and a hash of the binding token in constant time before
reading or changing a proposal; the binding is account and session scoped.
The token is never accepted in a JSON body, path, query, cookie, URL, or
model-visible result. Proposal create and commit also require
`Idempotency-Key` equal to `operation_id`; cancel requires it equal to
`cancel_request_id`. The bound observation GET omits
`Idempotency-Key`. Missing, duplicated, comma-joined, or conflicting security
headers fail closed.

The human page is a separate trust boundary at exactly
`https://auth.hello.food`. It establishes a fresh, independently
authenticated, host-only `__Host-heyfood-agent-approval` cookie with
`Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/`, and no `Domain`. The
proposal GET issues a new `decision_nonce` and CSRF token bound server-side to
that cookie, account subject, approval reference, proposal digest, and expiry.
The decision POST requires:

```text
Content-Type: application/json
Origin: https://auth.hello.food
Sec-Fetch-Site: same-origin
X-Heyfood-CSRF-Token: <43-character base64url token>
```

It also requires the authenticated cookie and matching one-use
`decision_nonce` in the strict `human_decision_request` body. The page rejects
missing or conflicting `Origin`, cross-site Fetch Metadata, query-supplied
CSRF or decisions, stale/non-matching cookie state, alternate redirect
targets, and reused nonces. Responses use `Cache-Control: no-store`,
`Referrer-Policy: no-referrer`, a nonce-based script policy, and
`frame-ancestors 'none'`. A host permission prompt, MCP elicitation result,
model text, form-mode response, or browser-open event never substitutes for
this decision POST.

## Versioned wire envelopes

| Step | Request/body | Result |
|---|---|---|
| Create session | `session_create_request` plus account auth and matching idempotency header | `session_created`; its binding token stays inside locked MCP process memory |
| Freeze proposal | `proposal_create_request` plus bound MCP headers and matching `operation_id` idempotency header | `proposal_created` with internal `approval_id`, allowlisted `presentation`, and fixed-origin `approval_url` |
| Render human page | No JSON body; authenticated cookie and exact path reference | `human_proposal_view` for page bootstrap only; never returned to MCP/model |
| Record decision | `human_decision_request` plus the exact browser headers/cookie above | `human_decision_result` |
| Observe/reconcile | No JSON body; bound MCP headers | `approval_observation` |
| Commit | `commit_request` plus bound MCP headers and matching `operation_id` idempotency header | `commit_receipt` or an observation/error requiring reconciliation |
| Cancel | `cancel_request` plus bound MCP headers and matching `cancel_request_id` idempotency header | `cancellation_receipt` |
| Any failure | N/A | strict `protocol_error`; never an HTML body or untyped success |

`approval_id` is internal transport state. The MCP implementation retains it
beside the binding token and uses it to observe, commit, or cancel. It is not
included in `AgentProposalPresentation` or an MCP tool result.

## Backend records and endpoints

MCP endpoints use the production API origin and both binding headers where
shown. Human endpoints use the authentication origin and the browser boundary
above.

| Origin, method, and path | Caller | Effect |
|---|---|---|
| `https://api.hello.food` `POST /v1/agent-approval/sessions` | local MCP | Consume `session_create_request`; create and return `session_created` |
| `https://api.hello.food` `POST /v1/agent-approval/proposals` | bound MCP | Consume `proposal_create_request`; store one immutable proposal and return `proposal_created` |
| `https://auth.hello.food` `GET /agent-approval/{approval_reference}` | authenticated human page | Return `human_proposal_view` for the exact stored proposal and matching account |
| `https://auth.hello.food` `POST /agent-approval/{approval_reference}/decision` | authenticated human page | Consume `human_decision_request`; compare-and-swap one decision and return `human_decision_result` |
| `https://api.hello.food` `GET /v1/agent-approval/approvals/{approval_id}` | bound MCP | Return `approval_observation` for status or reconciliation |
| `https://api.hello.food` `POST /v1/agent-approval/approvals/{approval_id}/commit` | bound MCP | Consume `commit_request`; atomically consume one approval and return `commit_receipt` |
| `https://api.hello.food` `POST /v1/agent-approval/approvals/{approval_id}/cancel` | bound MCP | Consume `cancel_request`; cancel without product mutation and return `cancellation_receipt` |

The approval URL is `https://auth.hello.food/agent-approval/{approval_reference}`.
It contains no account identifier, proposal data, digest, decision, session
identity, binding token, commit token, bearer credential, or redirect target.
Query parameters and alternate origins are rejected.

## Errors, conflicts, replay, and retry

Every error is the schema's strict `protocol_error`; success HTTP statuses
never carry an error envelope, and error statuses never carry a success
envelope. Error codes map as follows:

| Code | HTTP | Meaning and next action |
|---|---:|---|
| `invalid_request` | 400 | Reject before state change |
| `unauthenticated` | 401 | Account authentication missing/invalid; no retry inside the operation |
| `forbidden` | 403 | Binding, account, session, CSRF, origin, or browser trust mismatch |
| `approval_not_found` | 404 | No matching record within the authenticated and bound namespace |
| `approval_conflict` | 409 | Digest, operation, state, idempotency payload, or decision replay conflicts |
| `approval_expired` | 410 | Terminal expiry; prepare a new proposal only as a new user operation |
| `rate_limited` | 429 | No automatic retry; any delay hint is handled outside this operation |
| `internal_before_dispatch` | 500 | Server proves no state transition began; still no automatic retry |
| `outcome_uncertain` | 503 | Dispatch may have occurred; reconcile by observation/resource state |

All errors set `retry_allowed: false`. Only `outcome_uncertain` may set
`outcome_uncertain: true`, and it identifies `observe_approval` or
`observe_resource_state`. No error includes binding tokens, CSRF tokens,
cookies, bearer credentials, proposal internals, stack traces, or whether a
record exists outside the authenticated account/session namespace.

An idempotency-key replay with byte-identical RFC 8785/JCS request content
returns the original result. Reuse with different content returns
`approval_conflict`. An identical already-recorded human decision returns the
original `human_decision_result`; a conflicting decision or reused
`decision_nonce` returns `approval_conflict`. Committed, declined, cancelled,
expired, and invalidated records are terminal. Clients never retry a POST
after dispatch without first executing the prescribed observation.

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
