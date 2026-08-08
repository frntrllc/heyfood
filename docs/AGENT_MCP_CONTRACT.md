# heyfood local MCP contract

**Status:** v0.9.0 ten-tool read/discovery contract; the v0.8.0 eight-tool
inventory remains frozen in manifest schema v3

The supported `v0.9.0` release uses this ten-tool contract. The prior `v0.8.0`
release remains on its frozen eight-tool schema-v3 surface.

## Transport

The initial server is local stdio:

```text
/absolute/verified/path/heyfood mcp serve
```

It is the sole long-lived exception to the ordinary one-JSON-value CLI
contract. Before protocol startup, human-output modifiers such as `--json`,
`--raw`, `--no-color`, and `--no-banner` are rejected. After startup:

- stdout contains only valid MCP JSON-RPC frames;
- diagnostics remain privacy-safe on stderr;
- protocol errors use JSON-RPC/MCP errors;
- EOF cancels all work and exits within five seconds; and
- the process never detaches from its parent stdio connection.

## Environment and credential isolation

MCP mode uses
`docs/release-evidence/agent-native-phase0/mcp-environment-policy.json`.
Before reading state or credentials, it rejects every inherited environment
variable whose name starts with `HEYFOOD_`. It then constructs its service and
credential configuration without calling the ordinary CLI environment
readers:

- service origin is the compiled `https://api.hello.food` constant under the
  production network policy;
- API-key environment fallback is disabled;
- only the account-bound native credential backend is accepted;
- legacy/file credential fallback is disabled; and
- the state root comes from the reviewed platform default, not an inherited
  path.

The host registration contains no environment entries. The setup receipt
records the exact empty environment and the environment-policy digest. A
different environment, policy digest, executable, host, or scope blocks
receipt-based replacement/uninstall and reports a conflict.

Qualification proves that API-origin, API-key, CA, credential-store,
state-root, debug/test, and unknown `HEYFOOD_*` substitution fail before
credential access, network dispatch, or stdout protocol startup.

## Initial tool set

The server advertises exactly these read/discovery tools:

```text
heyfood_get_manifest
heyfood_get_status
heyfood_get_capabilities
heyfood_get_grocery_list
heyfood_get_grocery_exclusions
heyfood_list_menu_watches
heyfood_get_household_context
heyfood_get_household_member
heyfood_list_diets
heyfood_get_diet
```

The two Diet tools are authenticated, read-only application surfaces. They
rediscover capabilities for every call and dispatch only when the deployment
advertises exactly `diet:v1`; missing and unknown versions fail closed. The
catalog accepts bounded pagination. Detail accepts one exact, case-sensitive
`diet_id`. Neither tool sets or clears a profile diet.

The two household tools are local, account-bound reads. They call the same
application controller as the one-shot household commands, never acquire the
remote-operation semaphore, and never perform hosted dispatch. Context reads
are limited to content-free or roster projection. Minimized profile disclosure
requires `heyfood_get_household_member` with an exact stable additional-member
reference and a current disclosure generation; self-profile reads are not
supported.

There is no generic shell, command runner, arbitrary URL fetch, raw API proxy,
credential read, file read, or TUI-control tool.

Mutation candidates remain absent until Phase 4 individually qualifies their
full state machine. Tool names in planning documents are not advertisements.

## Bounds

| Resource | Maximum |
|---|---:|
| Inbound JSON-RPC frame | 1 MiB |
| Encoded tool arguments | 1 MiB |
| Structured result before host framing | 4 MiB |
| SSE line | 64 KiB |
| SSE event | 1 MiB |
| Normalized conversational stream | 4 MiB or 4,096 events |
| Outstanding JSON-RPC requests | 8 |
| Authenticated remote operations in flight per account-bound process | 1 |
| Bounded queued requests | 7 |
| Records per page | 100 |

The ninth request receives a typed overloaded error and is not queued.
Network-free manifest and household reads may run while one remote operation
is in flight, but total outstanding work remains eight. Slow readers cannot create
an unbounded channel. The transport admits the initialization notification
once and at most one cancellation notification for each active request.
Duplicate lifecycle notifications and client notification classes this server
does not consume are dropped before the SDK can spawn handler work.

The four hosted collection tools accept a closed optional input object with
`limit` (1 through 100) and an opaque, snapshot-bound `cursor`. Results include
a `page` object with `returned` and `next_cursor`. Each page is a fresh
authenticated read; changed collection bytes produce `mcp_cursor_stale`, and
callers restart from the first page rather than combining snapshots.

## Cancellation and retry

Queued and in-flight operations are cancellable. Cancellation before dispatch
is a safe non-operation. Once an uncertain POST may have been dispatched, the
tool returns a typed uncertain outcome and a bounded reconciliation action; it
never retries automatically.

EOF and parent death cancel all outstanding work. The server joins owned work
within the shutdown deadline and leaves no credential journal or child process
behind.

## Tool results

- Results are structured, versioned, bounded, and renderer-neutral.
- Every advertised JSON Schema is closed and contains both the tool's success
  document and the common privacy-safe error document. Runtime validation
  converts out-of-contract success data into `mcp_output_schema_mismatch`
  before writing it.
- Stable resource IDs, household/safety context, freshness, and provenance are
  preserved where required.
- Service failure is not converted into an empty list or success.
- Account data never appears in diagnostics or evidence.
- Credential rotation is separately declared from product-state mutation.
- Manifest and household tools are annotated read-only because they change
  neither product nor environment state. Hosted reads are not annotated
  read-only because credential rotation can persist authentication state.

## Required conformance

Qualification covers split/coalesced frames, concurrent requests, malformed
JSON-RPC, invalid UTF-8, oversized input/output, slow readers, floods,
queued/in-flight cancellation, overload, pagination, EOF, parent death, panic,
stdout/stderr isolation, auth/scope denial, service failure, redaction, prompt
injection, and process cleanup.
