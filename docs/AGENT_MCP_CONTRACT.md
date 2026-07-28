# heyfood local MCP contract

**Status:** Phase 0 protocol freeze; no `heyfood mcp` command is public

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

Phase 3 must prove that API-origin, API-key, CA, credential-store, state-root,
debug/test, and unknown `HEYFOOD_*` substitution fail before credential
access, network dispatch, or stdout protocol startup.

## Initial tool set

Phase 3 may advertise only these read/discovery candidates after exact schema
review:

```text
heyfood_get_manifest
heyfood_get_status
heyfood_get_capabilities
heyfood_get_grocery_list
heyfood_get_grocery_exclusions
heyfood_list_menu_watches
```

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
Network-free manifest/schema reads may run while one remote operation is in
flight, but total outstanding work remains eight. Slow readers cannot create
an unbounded channel.

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
- Stable resource IDs, household/safety context, freshness, and provenance are
  preserved where required.
- Service failure is not converted into an empty list or success.
- Account data never appears in diagnostics or evidence.
- Credential rotation is separately declared from product-state mutation.
- A tool is not annotated read-only unless the implementation proves it
  changes neither product nor environment state.

## Required conformance

Qualification covers split/coalesced frames, concurrent requests, malformed
JSON-RPC, invalid UTF-8, oversized input/output, slow readers, floods,
queued/in-flight cancellation, overload, pagination, EOF, parent death, panic,
stdout/stderr isolation, auth/scope denial, service failure, redaction, prompt
injection, and process cleanup.
