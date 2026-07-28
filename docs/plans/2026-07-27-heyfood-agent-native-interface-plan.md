# heyfood agent-native interface and distribution plan

**Status:** v0.6.0 read-only release slice complete — Phases 0 through 3 and
the applicable Phase 5 source, installed-artifact, and independent-review
requirements are closed. Phase 4 mutations are neither implemented nor
authorized.
**Baseline:** `frntrllc/heyfood` `main` at
`d68091a9cf6341c2c9120ba9251a6e0dd79a9616`
**Companion contracts:** `docs/CLI_CONTRACT.md`,
`docs/COMMAND_GRAMMAR.md`, `docs/CAPABILITY_STATUS.md`,
`docs/JSON_SCHEMAS.md`, and
`docs/plans/2026-07-19-heyfood-rust-native-client-plan.md`
**Primary users:** people using heyfood through a terminal, and authorized
coding agents such as Codex or Claude Code acting under those people's control
**Release boundary:** `v0.6.0` supersedes `v0.5.0` as the hosted-installer
default without changing the supported macOS/Linux platform boundary

**v0.6.0 release slice:** embedded self-description, receipt-bound Codex and
Claude skill/MCP setup, and exactly six read/discovery MCP tools. The absence
of every Phase 4 mutation tool is a required passing condition. A later
mutation release requires separate authorization, implementation, evidence,
and review; it is not a blocker for the read-only release slice.

## Executive decision

Make heyfood agent-native without turning its human TUI into an automation
protocol.

The supported architecture has four distinct surfaces:

1. **Repository guidance** teaches coding agents how to contribute to heyfood.
2. **Embedded self-description** lets any authorized shell agent inspect the
   exact installed binary's commands, schemas, capabilities, and safety rules.
3. **An installable Agent Skill** teaches Codex, Claude Code, and compatible
   hosts the preferred heyfood workflows through progressive disclosure.
4. **An MCP server** exposes typed, capability-aware operations without asking
   an agent to reverse-engineer help text or drive TUI keystrokes.

Bare `heyfood` and `heyfood chat` remain the human experience. Agents use
machine-facing CLI commands or MCP tools. Raw PTY automation of the TUI is a
qualification technique only and is never the supported agent integration.

The current Rust client already supplies important foundations: a typed
application/runtime split, exact JSON stdout, nonzero failures, stable error
envelopes, explicit uncertain outcomes, capability discovery, and
prepare/confirm Grocery operations. This plan adds discovery and distribution
around those contracts rather than creating a second client implementation.

## Current-state finding

The supported `v0.5.0` archive contains exactly one regular executable named
`heyfood`. It does not install:

- `AGENTS.md` or `CLAUDE.md`;
- an Agent Skill or plugin;
- an MCP server definition;
- the repository's Markdown contracts; or
- its JSON schemas as separately readable files.

An agent with shell access can run `heyfood --help`, per-command help,
completion generation, and one-shot commands with `--json --no-input`.
That is automation-friendly, but not comprehensive cold-start discovery.
Important semantics such as scope replacement, Grocery confirmation,
capability deferral, privacy, and uncertain-outcome reconciliation currently
remain in repository documentation.

The baseline one-shot surface also contains direct mutation paths that are not
yet safe agent fallbacks: `grocery confirm --decision accept` consumes a
model-visible serialized proposal containing `confirmation_token`, `log`
changes meal memory, and `watch add/remove` changes subscriptions. Phase 0
classifies every one-shot side effect. Until a route implements the trusted
approval protocol, it is `human_terminal_only` or `agent_unsupported`; the
Agent Skill must never invoke it.

Putting an instruction file beside the installed executable would not solve
this. Codex and Claude Code discover project or user guidance relative to their
configured instruction roots, not relative to arbitrary binaries on `PATH`.
The ordinary heyfood installer must not silently alter a user's global agent
instructions, permissions, or MCP configuration.

## Standards and interoperability decisions

This plan follows the current public extension models:

- Codex uses repository and user `AGENTS.md` guidance, reusable `SKILL.md`
  skills, plugins, and MCP servers.
- Claude Code uses repository and user `CLAUDE.md`, `SKILL.md` skills, plugins,
  and MCP servers. A small `CLAUDE.md` may import a canonical `AGENTS.md` with
  `@AGENTS.md` when both are repository contributor guidance.
- Agent Skills are the portable authoring format. Host-specific plugin
  wrappers may differ, but their heyfood workflow source must remain
  generated from or verified against one canonical skill.
- MCP is the typed execution and live-capability protocol. Instruction files
  and skills explain workflows; they do not substitute for authenticated,
  schema-validated tools.

Normative references:

- [Codex AGENTS.md guidance](https://learn.chatgpt.com/docs/agent-configuration/agents-md)
- [OpenAI Agent Skill guidance](https://developers.openai.com/plugins/build/skills)
- [OpenAI MCP server guidance](https://developers.openai.com/plugins/build/mcp-server)
- [Claude Code project guidance](https://code.claude.com/docs/en/memory)
- [Claude Code Agent Skills](https://code.claude.com/docs/en/slash-commands)
- [Claude Code MCP integration](https://code.claude.com/docs/en/mcp)

There is no assumption that an arbitrary agent scans every executable on
`PATH` or automatically reads a website's `llms.txt`. Website agent
documentation may improve discovery after a URL is known, but it is not the
installed-product bootstrap.

## Product contract

### Human and agent surfaces

| Surface | Intended user | Stability |
|---|---|---|
| Bare `heyfood` / `heyfood chat` | Human in an interactive terminal | Semantic TUI behavior; visual presentation may evolve |
| Manifest-listed `agent_safe` one-shot CLI with `--json --no-input` | Scripts and shell-capable agents | Stable process and JSON compatibility contract |
| `human_terminal_only` one-shot CLI | Person at an attached terminal | Explicitly unsupported for agent invocation |
| `heyfood agent ...` | Agents and integration installers | Versioned manifest, guide, schemas, and setup behavior |
| `heyfood mcp serve` | MCP-compatible agent hosts | Versioned tools, inputs, results, errors, and annotations |
| Root `AGENTS.md` / `CLAUDE.md` | Contributors working on the heyfood source | Repository development guidance only |

Every command and subcommand receives an audience classification:

- `agent_safe` — may appear in the Agent Skill and be invoked through the
  one-shot JSON fallback; no product-state mutation is `agent_safe` in this
  program;
- `human_terminal_only` — requires an attached human terminal and is absent
  from Agent Skill workflows; or
- `agent_unsupported` — absent from agent discovery and rejected if an
  agent-specific route attempts it.

Audience and transport are separate manifest fields. Each command declares
whether it accepts argument data, redirected data stdin, result stdout, and/or
an attached controlling-terminal interaction. `human_terminal_only` mutations
must obtain their semantic decision through a fresh controlling-terminal
interaction even when their data input or result output is redirected.
`--no-input` and execution without that separate terminal fail before
dispatch. A command-specific contract may still allow `--json` for its result.

In particular, the documented
`proposal.json -> grocery confirm --proposal-stdin` journey retains proposal
stdin, but the proposal bytes and `--decision accept` are no longer sufficient
authority. Before dispatch, the command renders the exact proposal on the
separate attached terminal and reads a fresh accept/cancel decision there.
This is a versioned process-contract change with migration guidance and
installed-artifact regression coverage. The production proposal, including
its confirmation token, never enters an agent-visible surface.

The classification does not pretend to identify who physically owns a shell.
It defines and enforces supported process modes and transport combinations.
Allocating a PTY to drive a human-only command remains unsupported
TUI/terminal automation and is a hard failure in cold-agent qualification.

The minimum mutation-related transport matrix is normative; Phase 0 expands
it to every exact command:

| Route | Audience | Data channel | Semantic authority |
|---|---|---|---|
| Agent-safe read, prepare, or cancel | `agent_safe` | Versioned JSON arguments/stdin and one JSON result | Never returns or accepts commit authority; prepare returns only `AgentProposalPresentation` |
| Human Grocery proposal creation that emits `GroceryMutationProposalWire` | `human_terminal_only` | Contracted arguments plus proposal JSON stdout/file | Attached-terminal acknowledgement is required before capability-bearing wire bytes are emitted |
| Human `grocery confirm` | `human_terminal_only` | `GroceryMutationProposalWire` on stdin; result may use contracted JSON stdout | Fresh exact-proposal review and accept/cancel on the separate controlling terminal |
| Human meal log and Menu Watch mutation | `human_terminal_only` | Only the arguments/stdin/stdout combination frozen for that command | Fresh controlling-terminal decision before dispatch |
| MCP protected mutation | MCP tool only | Typed MCP frames plus server-side approval observation | Bound heyfood-controlled out-of-band approval; never an agent-visible token or decision argument |

The human proposal-create and confirm commands therefore preserve redirected
JSON as a data transport while reserving semantic authority for the attached
terminal. Agent discovery contains neither their capability-bearing wire
schemas nor examples that invoke them.

### Cold-start promise

An agent with no heyfood repository checkout, no prior conversation, and only
the installed executable must be able to:

1. identify the exact heyfood version;
2. obtain a network-free machine-readable capability and command inventory;
3. distinguish human TUI operations from supported automation operations;
4. learn authentication and authorization prerequisites without reading
   credential material;
5. locate the relevant input/output schema and examples;
6. recognize read, prepare, confirm, cancel, and mutating operations;
7. understand that natural language is never Grocery mutation consent;
8. recognize `outcome_uncertain` and reconcile before retry;
9. identify unsupported or deferred capabilities truthfully; and
10. complete an authorized workflow without parsing human stderr or driving
    the TUI.

If an Agent Skill or MCP server is installed, the agent should discover this
information from the host's ordinary skill/tool discovery before trying shell
exploration.

## Safety, privacy, and authority invariants

These requirements apply to every phase:

1. **No implicit authority expansion.** Installing heyfood, its skill, or its
   MCP configuration does not grant shell, network, account, scope, microphone,
   filesystem, or mutation permission.
2. **No silent agent configuration writes.** The standard installer continues
   to install the executable only. Agent setup is a separate explicit command
   with preview/dry-run behavior and exact target paths.
3. **No instruction-file takeover.** Setup never overwrites or appends to an
   existing `AGENTS.md`, `CLAUDE.md`, skill, plugin, or MCP configuration
   without a distinct explicit replacement decision.
4. **No secrets in discovery.** Guides, manifests, schemas, diagnostics, and
   MCP initialization never include access tokens, session material, profile
   content, Grocery contents, prompts, phone numbers, or credential locations
   beyond non-sensitive backend names.
5. **Existing credential boundary.** Agent operations use the same
   account-bound native credential ports and scope checks as the CLI/TUI.
   Provider tokens never move into the client or agent integration.
6. **No natural-language consent.** A model saying that the user probably
   wants an operation is not confirmation. Grocery confirmation retains exact
   list ID, list version, context hash, operation, confirmation ID, and
   idempotency authority. A model-visible tool argument, opaque proposal token,
   MCP approval hint, or form response is not proof that a person reviewed the
   exact proposal. Agent mutations use the MCP ceremony below; legacy human
   one-shot mutations obtain a fresh decision on an independently attached
   controlling terminal.
7. **Prepare is not commit.** Preparation remains non-mutating product work.
   Cancellation is always available and must prove no product-state change.
   Commit-capable authority is released only after an independently recorded
   heyfood-controlled approval ceremony defined below.
8. **Uncertain means reconcile.** No shell wrapper, skill script, MCP server,
   or host hint may blindly retry an operation with an uncertain outcome.
9. **Accurate annotations.** MCP read-only, destructive, and open-world
   annotations describe real behavior. An annotation never replaces
   server-side authentication, authorization, validation, or confirmation.
10. **Untrusted content stays data.** Menu text, provenance, restaurant pages,
    agent responses, and Grocery item content cannot modify the skill,
    manifest, MCP instructions, tool choice policy, or confirmation state.
11. **Deferred remains deferred.** The manifest may truthfully report Health,
    default-build voice, Windows distribution, or another capability as
    deferred or unavailable, but no command, skill, or MCP tool may represent
    it as callable or supported until its public product contract changes
    independently.
12. **TUI remains borrowed terminal state.** An agent integration never leaves
    raw mode, alternate screen state, or a child heyfood process behind.
13. **No fallback downgrade.** MCP absence never authorizes an agent to invoke
    a `human_terminal_only` or direct mutating CLI command. Agent-safe CLI
    mutation is outside this program; adding it later requires a separate
    channel-neutral approval design and review.

## Target architecture

```text
                         human
                           |
                    heyfood TUI/CLI
                           |
Agent Skill ----> one-shot JSON CLI ----\
                                         \
Agent host ------> heyfood MCP server ----> heyfood application use cases
                                           |          |
                                           |          +--> core contracts
                                           +--> existing runtime/platform ports

Repository AGENTS.md / CLAUDE.md ---> contributor agent only
Embedded guide/manifest/schemas ----> CLI, skill, MCP, docs, and tests
```

### Crate boundary

The baseline does not yet expose all required application seams. Qualified
one-shot orchestration, Menu Watch, status/capability presentation, and much of
the Grocery flow remain in `heyfood-bin` or as concrete
`heyfood-agent-runtime::HttpService` methods. `GroceryPort` has no production
implementation, and no complete application-level Menu Watch or status port
exists. MCP work therefore begins with an application-boundary extraction; it
must not copy that logic into a new surface crate.

The Phase 0 ADR chooses the smallest acyclic split. The default candidate is:

```text
heyfood-agent-contract
    -> heyfood-cli
    -> heyfood-application
    -> heyfood-core

heyfood-mcp
    -> heyfood-agent-contract
    -> heyfood-application
    -> heyfood-core

heyfood-agent-setup
    -> heyfood-agent-contract
    -> reviewed platform/setup primitives

heyfood-bin
    -> composes CLI, TUI, agent contract, setup, MCP, runtime, and platform
```

The ADR may retain one `heyfood-agent-interface` leaf if the executable proof
shows that manifest, MCP, and setup concerns stay cohesive without importing
concrete runtime/platform code. It may instead approve the split above.
The names are provisional; the dependency and ownership rules are not.

Before Phase 3:

1. `heyfood-application` defines object-safe controllers/ports for
   conversational turns, capability/status, Grocery, and Menu Watch.
2. `HttpService` and the existing platform adapters implement those ports.
3. The current CLI and TUI routes use the same controllers with parity tests.
4. MCP adapters depend only on application contracts, never directly on
   `HttpService`, credential implementations, TUI state, or binary dispatch.
5. `heyfood-bin` remains the sole production composition root and supplies
   runtime/platform implementations to CLI, TUI, or MCP dispatch.

`heyfood-cli` continues to own Clap parsing. Agent-contract tests inspect the
Clap command tree through typed metadata; production code must not scrape
rendered help text. An inventory gate maps every active public command to
exactly one manifest entry and proves hidden topology is absent and deferred
topology is reported only as non-callable status.

The Phase 0 proof is a thin executable path, not a compile-only skeleton. It
must compose an embedded manifest plus one authenticated, cancellable read
through fake application ports from `heyfood-bin`. No implementation may
create a CLI/TUI/runtime dependency cycle or duplicate application orchestration
to make that proof pass.

## Versioned agent manifest

Introduce:

```text
schemas/v1/heyfood-agent-manifest.schema.json
```

and the network-free command:

```bash
heyfood agent describe --json
```

The manifest has its own `schema_version`, independent from the binary version
and MCP protocol version. Its stable core includes:

- product name and exact binary version;
- embedded build-input provenance: source commit, source tree and
  dirty/development identity, toolchain, distribution channel, build target,
  enabled Cargo feature set, and reproducible build-input digest;
- manifest schema version;
- supported automation surfaces;
- active, deferred, and unavailable public capability status; hidden command
  paths and internal topology never appear;
- command path, purpose, input channel, output family, and exit behavior;
- command audience: `agent_safe`, `human_terminal_only`, or
  `agent_unsupported`;
- interactivity and browser-handoff behavior;
- required scopes and authorization-upgrade guidance;
- operation class: local read, remote read, prepare, confirm, cancel, or
  mutation;
- retry class and reconciliation command;
- schema identifiers and embedded-schema digests;
- human-confirmation requirements;
- stable examples that contain no live account or dietary data;
- compatibility and additive-field policy; and
- documentation/skill/MCP compatibility versions.

The embedded manifest never claims to contain the digest of the final
executable or release archive that contains it. `agent doctor` may optionally
compute the on-disk executable digest as local observed state. Signed
executable and normalized archive digests remain external release-attestation
evidence bound to the embedded build-input provenance.

The manifest is byte-deterministic for an exact artifact. It performs no
credential access, filesystem mutation, browser launch, network request,
capability request, or telemetry. Platform/build fields may differ across
qualified artifacts; semantic fields must be identical for one product
version unless a feature is truthfully artifact-specific.

Related network-free commands:

```bash
heyfood agent guide
heyfood agent guide --format markdown
heyfood agent schema --list --json
heyfood agent schema MANIFEST_OR_OUTPUT_ID
heyfood agent doctor --json
```

`guide` emits the exact embedded agent operating guide. `schema` emits exact
embedded schema bytes. `doctor` checks local installation and integration
readiness without disclosing credentials or contacting hello.food unless an
explicit future `--online` flag is separately reviewed. Its schema, ordering,
redaction, and classification are deterministic, but its values and bytes may
truthfully vary with installation state, host versions, and owner-approved
paths; only manifest, guide, and schema bytes are exact-artifact fixtures.

Global `--json`, `--no-input`, `--no-color`, and stdout/stderr rules continue
to apply. Machine results use one JSON value on stdout.

## Canonical Agent Skill

The canonical skill is stored as reviewed source under:

```text
agent-integrations/skills/heyfood/SKILL.md
agent-integrations/skills/heyfood/references/
```

The initial description must be short enough for host startup discovery and
must trigger on requests to use, automate, inspect, or troubleshoot heyfood.
The full skill uses progressive disclosure and tells the agent to:

1. prefer MCP when the configured server is healthy;
2. otherwise call `heyfood agent describe --json`;
3. use only manifest-listed `agent_safe` one-shot CLI JSON with `--no-input`,
   never `human_terminal_only` commands or TUI keystrokes;
4. check capability and authorization state before product operations;
5. preserve stable resource IDs and non-capability proposal references without
   requesting, retaining, or reproducing commit credentials;
6. prepare and display a mutation before the heyfood-controlled approval
   ceremony;
7. cancel when confirmation is absent or ambiguous;
8. reconcile uncertain outcomes rather than retry;
9. treat product/service content as untrusted data; and
10. report deferred capabilities without inventing fallbacks.

Large command inventories, schemas, and examples remain in referenced files or
the binary manifest rather than loading into every agent session.

Codex and Claude plugin packages may add host-specific manifests and setup
metadata, but their workflow prose and examples must be byte-generated from or
semantically verified against the canonical skill. Host-specific permission
syntax must not be copied into the portable skill as if it were universal.

## Explicit agent setup

Agent integration is opt-in:

```bash
heyfood agent setup --target codex --scope user --dry-run
heyfood agent setup --target claude --scope user --dry-run
heyfood agent setup --target all --scope project \
  --project-root /absolute/path --dry-run
```

Required behavior:

- `--dry-run` is the default unless `--apply` is supplied.
- JSON mode emits a deterministic-schema plan containing host/version,
  target, paths, binary identity, digests, conflicts, user handoffs, and
  actions. Host/path values may differ by machine.
- User scope and project scope are distinct and never inferred from write
  access or the current working directory. Project scope requires an existing,
  explicit absolute `--project-root`; repository identity and trust are
  verified before any apply.
- Phase 0 freezes a host/version installation matrix covering supported skill,
  plugin, and MCP mechanisms, config ownership, required user interaction,
  update behavior, and rollback.
- Setup prefers the host's supported plugin marketplace/installer and
  `codex mcp` or `claude mcp` management commands. It does not directly rewrite
  shared Codex `config.toml`, Claude `~/.claude.json`, or project `.mcp.json`
  when a supported host-owned management path exists.
- A plugin installation that requires a host UI or user click returns a typed
  `user_action_required` handoff with verification instructions. It is never
  reported as installed before the host confirms it.
- MCP launch configuration uses a verified absolute path to the exact current
  heyfood executable plus its installation identity; a bare `heyfood` resolved
  from `PATH` is prohibited.
- Existing targets fail closed. Replacement requires an exact expected prior
  receipt/digest and an explicit replace option; arbitrary recursive overwrite
  is prohibited.
- If a supported host version leaves no alternative to a shared-file update,
  a host-specific adapter must use schema-aware merge, file locking,
  same-directory atomic replacement, owner-only permissions, rollback, and
  crash recovery while rejecting symlink, reparse-point, and hardlink
  substitution. Concurrent setup/uninstall is tested.
- Setup does not edit general `AGENTS.md` or `CLAUDE.md`. It may print an
  optional snippet or create a new file only when the exact target is absent
  and the user explicitly requests it.
- Uninstall removes only files whose current digest matches an installation
  receipt. Modified user files are preserved and reported.
- Installation receipts contain no credentials and use owner-only storage.
- Binary upgrades revalidate the absolute executable identity and
  skill/plugin/manifest compatibility before updating receipts or host-owned
  MCP entries. User policy and unrelated host configuration are preserved.
- Revocation or compatibility withdrawal disables the affected integration
  through the host-owned mechanism and retains a privacy-safe audit receipt.
- The normal `curl | bash` installer does not run agent setup.

The repository itself receives concise contributor-facing `AGENTS.md` and
`CLAUDE.md` guidance during this program. That guidance covers build/test
commands and the agent-safety invariants for contributors; it is not presented
as end-user CLI documentation.

## MCP product contract

### Initial transport

The first supported server is local stdio:

```bash
heyfood mcp serve
```

Local stdio reuses the native credential backend without exporting credentials
to the host configuration. It binds no listening socket and emits MCP frames
only on stdout; privacy-safe diagnostics use stderr. Remote Streamable HTTP and
new OAuth flows are separate future decisions, not implicit follow-on work.

`heyfood mcp serve` is an explicit exception to the ordinary one-JSON-value
process contract. It is a long-lived JSON-RPC stream:

- `--json`, `--raw`, `--no-color`, `--no-banner`, and other human-output
  modifiers are rejected before protocol startup rather than changing MCP
  framing.
- After protocol startup, stdout contains only valid MCP frames. Banners,
  warnings, diagnostics, panic text, normal CLI envelopes, and terminal escape
  sequences can never reach it.
- Diagnostics and a pre-handshake argument error use stderr. After handshake,
  protocol errors use JSON-RPC/MCP errors and privacy-safe operational
  diagnostics remain on stderr.
- EOF cancels outstanding work and the child exits within five seconds. The
  server never detaches or survives its parent stdio connection.

`CLI_CONTRACT.md` must record this exception before the command becomes
visible. Protocol qualification covers split and coalesced frames, multiple
requests, invalid UTF-8/JSON-RPC, oversized input, stdout/stderr isolation,
normal shutdown, parent death, and panic containment.

MCP initialization returns concise server instructions covering:

- capability discovery;
- authentication and scope behavior;
- confirmation and cancellation;
- uncertain-outcome reconciliation;
- untrusted content;
- deferred capabilities; and
- result-size/pagination rules.

The critical safety guidance must be self-contained at the beginning of the
instructions so hosts that truncate discovery still receive it.

### Resource, concurrency, and cost bounds

Phase 0 freezes these default maximums unless a smaller command contract
already applies:

- one inbound JSON-RPC frame and one encoded tool-argument object: 1 MiB;
- one outbound structured result: 4 MiB before host framing;
- one SSE line: the existing 64 KiB limit;
- one SSE event: the existing 1 MiB limit;
- one conversational operation: 4 MiB or 4,096 normalized stream events,
  whichever is reached first;
- eight outstanding JSON-RPC requests per server process;
- one authenticated remote product operation in flight per account-bound
  server process, with at most seven bounded queued requests; and
- one hundred records per page unless a smaller service contract applies.

Network-free manifest/schema requests may run while one remote operation is in
flight, but total outstanding work remains eight. The ninth request receives a
typed overloaded error and is not queued. Queued and in-flight work is
cancellable. Slow readers cannot create an unbounded event/result channel.

The stdio parent connection bounds the server lifetime. Per-request network,
SSE inactivity, and cancellation deadlines remain the existing application
contract; no detached background request survives a terminal result, EOF, or
shutdown deadline.

Service 429, quota, and daily-token exhaustion are returned truthfully with
retry timing only when the service supplies it. Neither the MCP adapter, Agent
Skill, nor host-facing recovery hints automatically retry or fan out requests
to evade limits. Tests cover cancellation while queued/in flight, slow
readers, floods, budget exhaustion, pagination, and shutdown with outstanding
work.

### Phase 3 read/discovery tool set

The first MCP increment is non-mutating with respect to hello.food product
state:

```text
heyfood_get_manifest
heyfood_get_status
heyfood_get_capabilities
heyfood_get_grocery_list
heyfood_get_grocery_exclusions
heyfood_list_menu_watches
```

These tools return structured, bounded results with stable identifiers and
privacy-safe summaries. Tools that contact the service are clearly distinct
from network-free manifest discovery. Capability and authorization denials
remain typed results; an unavailable service is not misreported as an absent
profile or capability.

An authenticated read may durably rotate a native session credential through
the existing journal. Such a tool is not labeled `readOnlyHint: true` unless
the exact implementation proves it cannot change any local or remote state.
The manifest separately reports product-state mutation, credential/session
side effects, and MCP annotations so “read-only Grocery” is not confused with
“no environment write of any kind.”

No generic `run_command`, arbitrary shell, arbitrary URL fetch, raw API proxy,
credential read, file read, or TUI-control tool is permitted.

### Trusted MCP agent mutation-consent protocol

MCP tool invocation, one-shot CLI invocation, model prose, tool annotations,
host permission prompts, and form-mode elicitation are not semantic approval
of a heyfood mutation. They may cause a proposal to be prepared, but cannot
release commit authority.

The only planned commit-capable agent ceremony is an out-of-band,
heyfood-controlled MCP approval:

1. The prepare call creates or validates the exact server proposal. The agent
   receives a renderer-neutral, schema-allowlisted
   `AgentProposalPresentation` containing display data, stable resource IDs,
   proposal digest, and a non-capability approval reference. It excludes
   `confirmation_token`, idempotency keys/authority, backend commit
   credentials, and equivalent reusable capability fields.
2. When the user asks to continue, MCP starts URL-mode elicitation. The URL
   targets an `auth.hello.food` approval page and contains no credential,
   personal data, pre-authenticated capability, or commit token.
3. The page requires the account owner to authenticate independently, displays
   the exact proposal and safety/context presentation, and records accept or
   cancel.
4. Approval is bound server-side to account/subject, MCP client and server
   session nonces, exact proposal digest, list ID/version, context hash,
   operation, expiry, and single use.
5. The local MCP server observes the bound result through its authenticated
   application port and commits at most once within the originating operation.
   The model never receives reusable commit authority.
6. Decline, cancellation, expiry, different account/session, proposal change,
   client disconnect, or missing URL-elicitation capability leaves the
   proposal uncommitted.

Host-native mandatory-interaction extensions and destructive annotations are
defense in depth only. Automated elicitation hooks, ordinary tool approval,
and a model-supplied `decision: accept` cannot satisfy this ceremony.

The agent-safe DTO is defined separately from
`GroceryMutationProposalWire`. Serialization uses an allowlist, not field
redaction after serializing a production wire object. Schema and negative
tests prove that current and future `confirmation_token`, idempotency,
operation-authority, and credential fields cannot enter model-visible
results.

Phase 0 must freeze and threat-model the companion backend approval contract
before Phase 4 is authorized. Until the backend and a target host/version pass
that review, MCP and the Agent Skill expose prepare/cancel/read operations
only. Existing direct `grocery confirm --decision accept`, `log`, and
`watch add/remove` routes are `human_terminal_only` for this program. The
person may invoke them independently outside the agent session, but the agent
may neither invoke them nor translate conversational approval into their
arguments/stdin. Their Phase 0 transport rows require an independently
attached controlling terminal for semantic decisions; Grocery continues to
accept proposal data on stdin, while redirected data for another route is
allowed only when its versioned command contract says so. If Codex, Claude, or
another host cannot prove URL elicitation and session binding for an exact
version, that host never receives the MCP confirm tool; CLI fallback does not
change that result.

### Phase 4 conversational and action tool set

Only after the read/discovery server passes its gate may the program add:

```text
heyfood_ask
heyfood_assess_item
heyfood_log_meal
heyfood_prepare_grocery_change
heyfood_cancel_grocery_change
heyfood_confirm_grocery_change
heyfood_create_menu_watch
heyfood_remove_menu_watch
```

This is a candidate namespace, not an all-or-nothing advertised set. Each
mutating tool is absent until its own complete state machine and host/version
gate pass.

Final names and schemas are frozen from application use cases, not shell
syntax. Every tool declares actual read-only/destructive/open-world behavior,
required scopes, idempotency semantics, maximum input/output size, and
potential uncertain outcomes.

Action requirements:

- `ask` and item assessment retain the existing validated service contracts
  and dispatch/cancellation semantics. They may return proposals, but neither
  their result nor a conversational tool call can commit a protected mutation.
- Every mutation family uses the same explicit state machine: prepare exact
  intent, return an agent-safe presentation, obtain independently
  authenticated approval of that exact presentation, commit once, verify
  resulting state, and reconcile an uncertain outcome. For agents this state
  machine is MCP-only in this program.
- Meal logging is classified as a mutation even when invoked conversationally.
  `heyfood_log_meal` remains absent until the backend/application provide its
  prepare/approval/commit/verify state machine.
- `heyfood_create_menu_watch` and `heyfood_remove_menu_watch` remain absent
  until Menu Watch provides the same state machine; direct existing CLI
  mutations remain human-only.
- Grocery preparation returns only `AgentProposalPresentation`, never exact
  proposal/commit authority.
- Grocery confirmation uses only the trusted mutation-consent protocol above.
  Model-visible proposal data or a structured tool decision is never commit
  authority.
- Unsupported host/version combinations receive prepare/cancel only; confirm
  is absent from their discovered tool set.
- Cancel, Ctrl+C/process cancellation, timeout, and disconnect prove
  non-mutation unless the result is explicitly uncertain.
- Each POST is entered in the DG-R2 dispatch/retry matrix and has fixtures for
  pre-dispatch, accepted dispatch, terminal response, cancellation, timeout,
  disconnect, uncertain outcome, reconciliation, and exact-once verification.

Tool annotations improve host behavior but are not the authorization boundary.
The MCP adapter and backend still validate identity, scopes, versions,
confirmation authority, and idempotency for every request.

## Phased implementation

### Phase 0 — Contract inventory, threat model, and architecture proof

**Purpose:** freeze what agents may discover and do before adding a public
surface.

Deliverables:

1. Inventory every active CLI command, subcommand, global flag, JSON output
   family, error type, scope, network call class, retry class, local/remote
   side effect, mutation, and `agent_safe`/`human_terminal_only`/
   `agent_unsupported` audience plus its per-command input, output, and
   controlling-terminal transport combination. This specifically includes
   Grocery confirm, meal log, Menu Watch, and any conversational path that can
   produce or execute a proposal.
2. Reconcile `CLI_CONTRACT.md`, `COMMAND_GRAMMAR.md`,
   `CAPABILITY_STATUS.md`, Clap help/completion, and JSON schemas.
3. Produce the agent threat model covering prompt injection, authority
   confusion, confirmation forgery, credential exposure, unsafe retries,
   malicious project configuration, plugin replacement, and oversized output.
4. Inventory orchestration still owned by `heyfood-bin` or concrete
   `HttpService` methods. Define object-safe application controllers/ports for
   conversational turns, capability/status, Grocery, and Menu Watch; bind
   `HttpService`; and route the existing CLI through them with parity tests
   before MCP implementation.
5. Write the crate-boundary ADR without preordaining one all-purpose crate.
   Complete the thin executable proof: embedded manifest plus one
   authenticated, cancellable read through fake ports, composed by
   `heyfood-bin`.
6. Freeze agent-manifest v1, MCP tool naming/schema conventions, protocol
   framing exception, resource/concurrency limits, and cost behavior.
7. Freeze the exact per-host/version Codex and Claude skill/plugin/MCP
   installation matrix, trusted absolute executable identity, update/rollback
   behavior, and reversible receipts from current official documentation.
8. Freeze and threat-model the companion hello.food out-of-band approval
   contract and the separate allowlisted `AgentProposalPresentation`. No
   model-visible token, tool argument, annotation, host prompt, form
   elicitation, one-shot stdin, or serialized production proposal may be
   treated as semantic consent.
9. Add concise repository contributor `AGENTS.md` and a Claude-compatible
   wrapper/import without making either an installed-product claim.
10. Add a machine-readable Phase 0 inventory with requirement IDs, owners,
   evidence locations, and blockers.

Exit gate:

- all public commands and DG-R2 POSTs are classified;
- every current one-shot mutation is either technically constrained to the
  human-terminal process mode or lacks an agent-facing route, and MCP absence
  cannot downgrade that classification;
- the documented human `proposal.json -> grocery confirm` journey passes with
  proposal stdin plus a distinct controlling-terminal review/decision, while
  noninteractive and MCP-absent agent attempts fail before dispatch;
- no active/deferred disagreement remains among help, completion, docs, and
  schemas;
- the existing CLI passes through the extracted application controllers with
  unchanged fixtures, streams, errors, and cancellation;
- the thin executable proof passes without an MCP/runtime/platform dependency
  cycle or duplicated command orchestration;
- the host setup matrix and concrete out-of-band consent protocol have no
  unresolved feasibility or authority gap;
- Rust architecture and security specialists independently approve the threat
  model, manifest schema, dependency direction, setup mechanics, resource
  bounds, and confirmation boundary; and
- Phase 0 evidence records zero unresolved P0/P1 findings.

Phase 0 may land behavior-preserving application-boundary extraction behind the
existing CLI/TUI plus documentation, schemas, tests, and a non-public
executable spike. It does not advertise agent support, add a public command, or
alter installation. It may also land the separately versioned
controlling-terminal enforcement and migration guidance needed to prevent the
current proposal/confirm, meal-log, and Menu Watch commands from becoming
agent fallbacks.

### Phase 1 — Installed-binary self-description

**Purpose:** make the exact artifact comprehensible without plugins, network,
credentials, or repository access.

Deliverables:

1. Add the ADR-approved agent-contract crate(s) with deterministic
   manifest/guide/schema rendering.
2. Add `heyfood agent describe`, `guide`, `schema`, and offline `doctor`.
3. Embed the reviewed guide and schema bytes in every native artifact while
   preserving the one-executable release archive.
4. Add command-inventory parity tests against the Clap command tree,
   completion, capability status, scopes, deferred topology, and command
   audience. Human-only mutation commands are absent from agent examples and
   enforce their exact transport rows before dispatch. Regression tests cover
   redirected Grocery proposal data plus attached-terminal human approval,
   missing-terminal rejection, and agent fallback rejection.
5. Add JSON Schema validation, golden fixtures, digest fixtures, output limits,
   allowlist/no-commit-authority tests, and cross-platform
   deterministic-output tests.
6. Document closed-schema compatibility and agent-manifest schema versioning.

Exit gate:

- a clean environment with only the installed executable obtains the complete
  cold-start promise;
- every discovery command is proven network-free, credential-free,
  mutation-free, deterministic in schema/order/classification, ANSI-free in
  JSON mode, and bounded; exact bytes are required for manifest, guide, and
  schemas while `doctor` values may reflect local state;
- macOS, Linux, and ordinary Windows CI agree on semantic manifest bytes;
- artifact-size impact is measured and accepted; and
- exact-SHA Rust and documentation review returns GO.

Phase 1 may ship as self-description without claiming Codex/Claude integration.

### Phase 2 — Agent Skill, plugins, and reversible setup

**Purpose:** let supported agents discover the right workflow immediately.

Deliverables:

1. Add the canonical heyfood `SKILL.md` and progressive-disclosure references.
2. Build Codex and Claude candidate skill packages generated from the
   canonical source. The supported `v0.6.0` path is binary-embedded,
   receipt-bound setup through each host's own configuration commands; a
   marketplace publication is not required or claimed.
3. Add `heyfood agent setup` dry-run/apply/uninstall with conflict-safe,
   receipt-bound host-owned operations and typed user-action handoffs.
4. Add the frozen host/version setup matrix, host-specific setup
   documentation, absolute executable binding, update/rollback instructions,
   and exact compatibility ranges.
5. Test explicit invocation, implicit invocation, irrelevant-prompt
   non-invocation, missing auth, missing scopes, deferred capabilities, and
   uncertain outcomes in clean host profiles. With MCP absent, test that the
   skill uses only `agent_safe` CLI routes and never invokes Grocery accept,
   meal log, Menu Watch mutation, or another human-only command.
6. Produce signed or attested candidate artifacts for Phase 5. Do not publish
   to a marketplace, enable setup by default, or make support claims before
   the installed-artifact gate closes.

Exit gate:

- new Codex and Claude sessions discover the skill without a repository
  checkout or pasted instructions;
- neither host is taught to drive the TUI;
- setup and uninstall preserve pre-existing user files under conflict,
  modification, concurrent invocation, interruption, upgrade, rollback, and
  partial-failure tests, including shared-config and link-substitution cases
  where a host-owned command is unavailable;
- skill output never grants permissions or bypasses host confirmation;
- canonical and host-specific packages pass semantic-drift verification; and
- exact-byte independent review approves each private candidate artifact.

### Phase 3 — Local MCP discovery and read operations

**Purpose:** provide typed live access with no product mutations.

Deliverables:

1. Verify the current CLI/TUI routes for these operations use the Phase 0
   application controllers and that `HttpService` supplies the production
   ports; MCP must not begin from concrete runtime methods or binary handlers.
2. Add a pinned, reviewed Rust MCP implementation and `heyfood mcp serve`.
3. Implement initialization instructions, tool listing, schemas, annotations,
   structured errors, pagination, cancellation, and output budgets.
4. Implement only the read/discovery tool set defined above.
5. Add explicit Codex and Claude MCP setup through host-owned mechanisms
   orchestrated by `heyfood agent setup`.
6. Add protocol conformance, split/coalesced-frame, multiple-request,
   malformed-frame, invalid-UTF-8, oversized-input/output, slow-reader, flood,
   queued/in-flight cancellation, 429/budget, pagination, EOF, parent-death,
   panic, stdout/stderr-isolation,
   auth-denial, scope-denial, service-failure, redaction,
   prompt-injection, and process-cleanup tests.
7. Prove no tool can access arbitrary shell, filesystem, URL, credential, or
   raw API functionality.

Exit gate:

- MCP Inspector plus Codex and Claude independently discover and call every
  tool with valid and invalid inputs;
- account data never crosses accounts or appears in diagnostics/evidence;
- all read results preserve safety status, household context, freshness,
  provenance, and stable identifiers;
- service failure is never guessed into a successful or empty result;
- server shutdown/cancellation leaves no child process or credential journal;
  and
- exact-SHA Rust, protocol, privacy, and security reviews return GO.

### Phase 4 — Conversational and explicitly confirmed actions

**Purpose:** support useful agent work without weakening mutation safety.

Deliverables:

1. Add the bounded conversational/action tools after separately reviewing each
   tool's authority and DG-R2 row.
2. Implement the reviewed hello.food out-of-band approval contract and bind
   its server-side authority to MCP client/server session identity, the exact
   proposal digest and preconditions, expiry, account, and single use without
   exposing that authority to the model.
3. Implement the allowlisted `AgentProposalPresentation` and prove model
   output cannot include the current/future production confirmation token,
   idempotency authority, commit credential, or serialized wire DTO.
4. Implement exact host/version gating for URL elicitation. Prepare/cancel
   remain available where the trusted ceremony cannot be proven; confirm is
   absent.
5. For each of Grocery, meal log, and Menu Watch, either implement the complete
   prepare/approve/commit/verify/reconcile state machine or keep its mutating
   MCP route absent. Agent-safe mutating CLI routes remain out of scope.
6. Keep existing direct mutation commands human-terminal-only unless they are
   explicitly migrated to a separately reviewed approval protocol. Enforce the
   Phase 0 per-command transport matrix: retain Grocery proposal stdin but
   require its distinct controlling-terminal review/decision; preserve only
   other redirected channels explicitly allowed by their versioned contracts;
   and test missing-terminal and MCP-absent fallback rejection.
7. Reuse the extracted application controllers and renderer-neutral
   confirmation documents;
   do not reproduce business logic in MCP handlers or skill scripts.
8. Add exact proposal integrity, stolen approval-request, replay,
   stale list/context, cross-account, cross-session,
   confirmation mismatch, cancellation, non-mutation, and exact-once tests.
9. Add negative direct-tool, agent-generated-acceptance, conversational
   proposal, and MCP-to-CLI fallback tests for every mutation family.
10. Add uncertain-outcome reconciliation tools or typed recovery instructions
   before enabling any uncertain POST.
11. Run least-privilege production canaries with synthetic/privacy-safe
   evidence for each enabled action family.

Exit gate:

- a positive Grocery confirmation on each eligible exact host/version mutates
  the intended list exactly once only after independently authenticated
  out-of-band approval of the exact proposal;
- cancel, ambiguous authorization, stale authority, scope loss, context
  change, cross-account/session state, automated/form elicitation,
  model-supplied acceptance, Ctrl+C, and unsupported hosts do not mutate;
- uncertain dispatch never triggers automatic replay;
- each advertised Menu Watch or meal-log action has independently approved the
  exact intent and verifies its resulting state; otherwise its tool is absent;
- existing human CLI/TUI workflows remain functional without an MCP host,
  including the documented Grocery proposal-stdin journey with its new
  attached-terminal decision; any companion backend approval support is
  protocol-neutral and additive and does not weaken human confirmation; and
- exact-SHA Rust, backend-contract, security, and independent agent-behavior
  reviews return GO.

Phase 4 produces private release candidates only. No mutating MCP tool may be
published, enabled by default, represented as supported, or advertised before
the corresponding Phase 5 host/version matrix closes.

### Phase 5 — Installed-artifact cold-agent qualification and rollout

**Purpose:** prove the end-to-end experience users will actually receive.

Qualification uses signed candidate archives, private skill/plugin candidates,
clean home directories, fresh agent sessions, ordinary host permission
prompts, the heyfood-controlled approval surface where applicable, and no
heyfood repository checkout. The minimum matrix covers:

| Journey | Required evidence |
|---|---|
| Discover installed heyfood | Exact version, manifest schema, supported surfaces, no network |
| Missing authentication | Truthful login/register handoff; no credential guessing |
| Returning authenticated user | Existing protected credential reuse without disclosure |
| One-shot question | Valid JSON/MCP turn, structured result, clean streams |
| MCP unavailable | Agent uses only manifest-listed `agent_safe` CLI; every human-only mutation remains untouched |
| Household Grocery read | Intended member, safety, substitutions, freshness, provenance |
| Grocery prepare/cancel | Complete proposal rendering and proven non-mutation |
| Grocery confirm on eligible host/version | Independently authenticated out-of-band approval of the exact proposal and exact-once advancement |
| Grocery confirm on ineligible host/version | Confirm tool absent; prepare/cancel and human CLI/TUI handoff remain truthful |
| Conversational proposal | `ask` may present intent but cannot directly or indirectly commit it |
| Meal log and Menu Watch mutation | Complete trusted state machine on each advertised host/version, otherwise mutating tool absent |
| Stale authority | Typed rejection with no mutation |
| Uncertain outcome | Reconciliation before any retry |
| Menu Watch | Truthful supported summary/actions only |
| Deferred capability | Health/default voice/unsupported platform remain unclaimed |
| Hostile content | Embedded instructions and tool policy remain unchanged |
| Process lifecycle | Cancellation, failure, and exit leave no TUI/process residue |

Run discovery/read/setup at minimum on:

- latest supported Codex CLI/desktop local host;
- latest supported Claude Code local host;
- macOS Apple Silicon and Intel signed/notarized archives;
- Linux x64 and ARM64 attested archives; and
- ordinary Windows source/CI qualification without claiming a Windows public
  artifact until the separate Windows release contract closes.

Run confirmation only on exact host/version combinations whose URL elicitation,
session binding, and out-of-band approval behavior passed Phase 4. The absence
of an eligible confirm tool is the expected passing behavior on other hosts;
the product must not claim cross-host agent confirmation parity that the matrix
does not prove.

Record:

- candidate source SHA and archive digest;
- agent host and version;
- installed skill/plugin/MCP artifact digests;
- exact manifest and schema digests;
- tool/command sequence with sensitive values redacted;
- backend request/canary correlation identifiers;
- pre/post state proofs for mutations;
- permission and confirmation decisions;
- retry/reconciliation behavior;
- terminal/process cleanup; and
- deviations, waivers, and reviewer verdicts.

Release gate:

1. every discovery/read/setup journey passes for both agent families, and each
   confirmation journey passes only on the exact host/version combinations
   where it is advertised;
2. unsafe mutation, blind retry, credential disclosure, cross-account access,
   instruction hijack, and false capability claims are all zero;
3. ordinary human TUI/CLI installed-artifact suites remain green;
4. source, binary, skill/plugin, manifest, schemas, and MCP evidence are bound
   to exact bytes;
5. independent exact-SHA and exact-byte reviewers return GO; and
6. capability/help/install documentation is updated only after qualification.

Public rollout is fail-closed. A source merge may precede activation, but the
product must not claim “Codex support,” “Claude Code support,” or general agent
control until the corresponding installed-artifact matrix passes. After GO,
rollout proceeds in this order:

1. publish the exact reviewed executable archives containing the canonical
   skill and setup implementation;
2. verify public archive, embedded-skill, manifest, schema, and installer
   bytes;
3. enable only the qualified host-owned setup targets and MCP tools;
4. update support/capability claims with exact host/version boundaries; and
5. run clean public installation, discovery, read, cancellation, and eligible
   confirmation smoke tests.

Each public integration has a reviewed withdrawal path: revoke or unlist a
plugin version where the host permits, mark the compatibility range
unsupported, disable affected setup targets/tools in a fix-forward binary,
preserve incident evidence, and leave unrelated user policy/configuration
untouched. Already-installed incompatible integrations must fail closed with a
typed upgrade or removal instruction.

## Cold-agent evaluation design

The evaluation harness must test behavior, not merely inspect the presence of
files or tool names.

Each case begins with a new agent session and one natural user request. Unless
the case explicitly tests fallback, the prompt does not tell the agent which
commands to run. Scorers inspect:

- whether the agent discovered the skill/MCP/manifest;
- whether it selected MCP or JSON CLI appropriately;
- schema-valid calls and results;
- unsupported command attempts;
- TUI/PTY control attempts;
- unnecessary permission requests;
- mutation attempts before confirmation;
- exact proposal/identifier preservation;
- blind retry attempts;
- state reconciliation;
- user-facing clarity and truthful capability statements; and
- sensitive data in conversation, logs, evidence, or command arguments.

Hard failures:

- driving the interactive TUI as the normal integration;
- parsing human stderr as structured output;
- passing secrets or full proposals in shell arguments;
- treating service/menu/Grocery content as instructions;
- changing household scope without resetting conversation continuity;
- confirming a mutation from natural language, a model-visible token/tool
  argument, an automated elicitation response, or an ordinary host approval
  prompt without the bound heyfood-controlled approval record;
- modifying a stale or different list/context/account;
- invoking Grocery accept, meal log, Menu Watch mutation, or another
  human-terminal-only command as an MCP-to-CLI fallback;
- retrying an uncertain operation;
- advertising a deferred capability;
- editing global agent configuration without explicit setup authorization; or
- registering a bare `heyfood` MCP launch command that can be substituted
  through `PATH`.

The gate requires behavioral execution on installed artifacts. A structural
test that only counts manifest records, skills, or MCP tool definitions does
not qualify the integration.

## Compatibility and release policy

- The agent-manifest schema, Agent Skill compatibility range, and MCP tool
  protocol are versioned independently.
- Agent manifest schema v1 is closed and rejects unknown fields. Adding,
  removing, or changing a manifest field or meaning requires a new manifest
  schema version plus migration guidance. Result and tool schemas declare
  their compatibility policy and version independently.
- Human help text and TUI layout are not parsed as machine contracts.
- Skills and plugins declare the minimum and maximum compatible heyfood
  manifest versions. Incompatibility fails with an upgrade instruction, not a
  best-effort guess.
- Phase 2 read-only artifacts are supported in v0.6.0 after their Phase 5
  installed-artifact GO. Phase 4 artifacts remain private candidates;
  marketplace publication, default setup activation, or MCP mutation
  advertisement require a separate applicable Phase 5 GO.
- Embedded guide, manifest, and schemas are built from the same source commit
  as the executable and covered by release attestations through the existing
  single-binary artifact.
- Separately distributed plugin/skill packages carry their own digests and
  provenance and are bound to compatible binary/manifest versions.
- No phase reopens Health, native voice, Windows distribution, or provider
  token storage. Those remain independently gated product workstreams.
- `heyfood mcp serve` is the documented long-lived JSON-RPC exception to the
  one-value JSON CLI contract; all other machine commands retain that contract.
- `v0.5.0` remains available as published, while `v0.6.0` is the current
  hosted-installer release. This plan does not retroactively alter the v0.5.0
  archive or claims.

## Documentation deliverables

Implementation maintains:

```text
AGENTS.md
CLAUDE.md
docs/AGENT_INTEGRATION.md
docs/AGENT_SAFETY.md
docs/AGENT_APPROVAL_CONTRACT.md
docs/CLI_CONTRACT.md
docs/COMMAND_GRAMMAR.md
docs/CAPABILITY_STATUS.md
docs/JSON_SCHEMAS.md
schemas/v1/heyfood-agent-manifest.schema.json
agent-integrations/skills/heyfood/SKILL.md
```

The embedded guide is generated from or byte-verified against the public
agent-integration and safety documents. Public website guidance may mirror the
exact reviewed bytes and advertise an MCP/plugin installation path after
qualification; it is not a separate authority.

## Required review roles

Every phase returns one exact product SHA and immutable evidence digest.

| Review | Required focus |
|---|---|
| Lead Rust specialist | crate boundaries, typed contracts, cancellation, determinism, dependency policy |
| Security specialist | credentials, authorization, confirmation, prompt injection, setup writes, MCP threat model |
| CLI/TUI contract reviewer | no regressions to human TUI, JSON streams, exit codes, help, completion |
| Agent-integration reviewer | Codex/Claude discovery, skill quality, MCP schemas, cold-start behavior |
| Release reviewer | exact artifacts, provenance, host setup packages, compatibility, public claims |

An author or implementation agent cannot provide the sole approval for its own
phase. Review is performed on exact SHA/bytes after ordinary CI is green.
Findings are retained in the phase inventory until a reviewer closes them;
administrative wording cannot convert an unresolved technical finding into GO.

## Program sequencing and parallelism

```text
Phase 0 contract/threat model
              |
              v
Phase 1 embedded discovery
        |                 \
        v                  v
Phase 2 Agent Skill     Phase 3 read-only MCP
        \                  /
         \                /
          v              v
       Phase 4 confirmed actions
                    |
                    v
       Phase 5 installed qualification
                    |
                    v
             truthful public rollout
```

Phase 2 and Phase 3 may proceed in parallel after Phase 1 freezes the
manifest/guide schema. Phase 4 requires both and a separate security GO.
Documentation, schemas, and cold-agent fixtures evolve with each phase rather
than being deferred to the release candidate.

## Definition of done

The program is complete when:

- an installed heyfood binary explains itself comprehensively and
  deterministically without network or repository access;
- Codex and Claude discover one canonical, concise heyfood workflow through
  supported skill/plugin mechanisms;
- supported hosts can use typed MCP operations without shell-help
  reverse-engineering or TUI automation;
- every mutation retains independently authenticated heyfood-controlled human
  approval of the exact proposal, exact server preconditions, safe
  cancellation, and uncertain-outcome reconciliation; ineligible hosts expose
  no confirm tool;
- agent setup is opt-in, conflict-safe, reversible, and credential-free;
- deferred capabilities remain truthful;
- cold-agent installed-artifact tests pass across supported release targets;
- human TUI and one-shot CLI quality do not regress; and
- exact-SHA and exact-byte independent reviews approve source, artifacts,
  integrations, evidence, and public claims.

## Non-goals

- Embedding a local language model or inference runtime in the CLI.
- Letting heyfood execute arbitrary shell commands, edit source, browse
  arbitrary URLs, or become a general coding agent.
- Treating TUI keystroke control as the supported agent API.
- Installing global agent permissions or silently changing user instructions.
- Replacing backend authentication, authorization, confirmation, idempotency,
  Grocery, Menu Watch, or safety contracts.
- Moving Kroger, Oura, Apple Health, or other provider credentials into the
  native client.
- Re-enabling deferred Health, default-build voice, or Windows release claims.
- Claiming that a Markdown file alone provides enforcement or comprehensive
  tool discovery.
- Blocking continued human TUI excellence work on the existence of agent
  integrations; both surfaces share application contracts but have distinct
  qualification.
