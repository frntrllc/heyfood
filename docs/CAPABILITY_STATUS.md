# Capability and distribution status

This page is the authoritative public boundary between the current Rust command
surface, hosted hello.food capabilities, and preview work. Recognition of a
hidden legacy command is not support: unfinished paths return
`command_not_available`, while the retained Health spelling returns the more
specific `capability_deferred`.

## Distribution

| Surface | Status | Meaning |
|---|---|---|
| Native v0.4.0 | Unsupported | Do not install or use. Published before release authorization. |
| Native v0.4.1 | Unsupported | Do not install or use. Published before release authorization. |
| Native v0.5.0 | Previous supported release | The qualified Rust recovery release superseded by the v0.6 line. |
| Native v0.6.0 | Previous supported release | Superseded by v0.6.1, which adds supported account-bound logout. |
| Native v0.6.1 | Current supported release | Retains exact installed self-description, receipt-bound skill/MCP setup, and six bounded read/discovery MCP tools; adds current-device authority revocation and local credential teardown through `heyfood logout`. |
| Hosted installer | Supported | Installs the checksum-verified native `v0.6.1` archive for macOS or Linux. |
| Source | Available | Public for inspection and contributor evaluation under Apache 2.0. |
| Windows x86-64 | Deferred | Windows distribution requires a separately qualified future release. Ordinary Windows compile, test, Clippy, Credential Manager, and deterministic packaging qualification remains active in CI, but no Windows credential or asset participates in the public release path. |
| macOS v0.6.1 archives | Supported | Both architectures are Developer ID signed with hardened runtime, Apple notarized, attested, and installed-artifact qualified. |
| Linux v0.6.1 archives | Supported | Both architectures are checksum verified, attested, and installed-artifact qualified. |
| Current native release | v0.6.1 | Supported on the four macOS/Linux targets listed above. |

## Product capabilities

| Capability | Rust CLI | Hosted hello.food | Status |
|---|---|---|---|
| Account connection | Bare `heyfood` | Account-neutral device authorization with hosted sign-in/create-account choice | Current source first-run flow |
| Registration | `heyfood register` | Explicit create-account device authorization, identity verification, agreements, and consent | Current source command |
| Login and authorization replacement | `heyfood login` | Fresh existing-account connection or atomic channel and application-session grant replacement | Current source command |
| Logout | `heyfood logout` | Resolves and revokes the current channel link, then revokes the current device and app session | Supported; local account-bound credentials are always cleared and interrupted two-store teardown is resumable |
| Food questions | `heyfood ask` | Hosted agent turn | Current source command |
| Conversation continuation | `heyfood reply --conversation-id …` | Hosted conversation state | Current source command |
| Meal logging | `heyfood log` | Hosted agent and meal memory; direct use requires controlling-terminal `LOG` authorization | Current human-terminal-only source command |
| Item evaluation | `heyfood item` | Restaurant/menu evidence and dietary evaluation | Current source command |
| Grocery | `heyfood grocery` plus TUI confirmation cards | Read/export; direct preparation and confirm/cancel require exact controlling-terminal review phrases | Supported v0.6.1 command with human-only mutations |
| Oura health integrations | Not advertised; retained command spelling fails closed with `capability_deferred` | Future provider-neutral integration work | Deferred from the supported `v0.6.1` contract; no implementation or canary release gate |
| Apple Health | No CLI command or TUI panel | Mobile/backend work remains outside this release | Deferred from the supported `v0.6.1` contract |
| Household context | Used by hosted turns and Grocery | Profiles and household-aware evaluation | Backend available; native roster management unavailable |
| Restaurants and recipes | Via `ask` and `item` | Resolution, menu evidence, and recipe tools | Hosted through current commands |
| Menu Watch | `heyfood watch` plus `/watch` TUI panel | List plus human-terminal-only create (`CREATE`) and remove (`REMOVE`); scheduled execution is deployed; latest account-owned change summary, source, freshness, and provenance render in the TUI | Current source management and summary view; item-level diff detail remains follow-on |
| Interactive TUI | Launches authenticated chat and functional read/action panels | N/A | Supported and installed-artifact qualified in v0.6.1. |
| Voice capture | TUI `/voice`, Ctrl+Space, and F8 only in opt-in `native-audio` artifacts | Authenticated transcription | Not enabled in the default `0.6.1` build; real-hardware and platform qualification remain future work. |
| Agent self-description | `heyfood agent describe/guide/schema/doctor` | None | Supported in v0.6.1; offline, credential-free, bounded, and non-mutating. |
| Codex/Claude Agent Skill setup | `heyfood agent setup` / `agent uninstall` | N/A | Supported in v0.6.1 for the exact qualified host versions; dry-run is default, apply requires the reviewed plan digest, and setup registers MCP through host-owned commands. |
| Local read-only MCP | `heyfood mcp serve` | Existing authenticated read controllers | Supported in v0.6.1 with exactly six bounded read/discovery tools, bounded cursor pagination, and no agent mutation surface. |

## Process contract

`--json` emits exactly one ANSI-free JSON value on stdout. Human diagnostics
and progress use stderr. Redirected UTF-8 stdin is accepted by `ask`, `reply`,
`log`, and `item`. Runtime failures use nonzero exit status and machine-readable
errors; uncertain write outcomes are explicit.

See [CLI_CONTRACT.md](CLI_CONTRACT.md) for the stable process interface.
