# Capability and distribution status

This page is the authoritative public boundary between the current Rust command
surface, hosted hello.food capabilities, and preview work. Recognition of a
hidden legacy command is not support: unfinished paths return
`command_not_available`, while the retained Health spelling returns the more
specific `capability_deferred`.

## Distribution

| Surface | Status | Meaning |
|---|---|---|
| Native v0.5.0 | Previous supported release | The qualified Rust recovery release superseded by the v0.6 line. |
| Native v0.6.0 | Previous supported release | Superseded by v0.6.1, which adds supported account-bound logout. |
| Native v0.6.1 | Previous supported release | Added account-bound logout but rejected local teardown after normal app-session rotation. Superseded by v0.6.2. |
| Native v0.6.2 | Previous supported release | Fixed current-device logout after normal app-session rotation. Superseded by v0.7.0. |
| Native v0.7.0 | Previous supported release | Added responsive dietary setup, complete slash-command discovery, terminal-native restaurant/menu presentation, typed slow-turn recovery, crash-safe logout, and the encrypted local household lifecycle. Superseded by v0.7.1. |
| Native v0.7.1 | Current supported release | Redesigns the interactive terminal with a calm responsive frame, branded startup view, bordered composer, and contextual working, location, household-scope, version, and release-channel cues while preserving the v0.7.0 capability boundary. |
| Hosted installer | Supported | Installs the checksum-verified native `v0.7.1` archive for macOS or Linux. |
| Source | Available | Public for inspection and contributor evaluation under Apache 2.0. |
| Windows x86-64 | Deferred | Windows distribution requires a separately qualified future release. Ordinary Windows compile, test, Clippy, Credential Manager, and deterministic packaging qualification remains active in CI, but no Windows credential or asset participates in the public release path. |
| v0.7.1 release asset set | Supported | Exactly four product archives, four matching standalone-verifier archives, one canonical native-state declaration, and `SHA256SUMS`; every asset is attested and verified again after public download. |
| macOS v0.7.1 archives | Supported | The product and verifier executables for both architectures are Developer ID signed with hardened runtime, Apple notarized, attested, and installed-artifact qualified. |
| Linux v0.7.1 archives | Supported | The product and verifier archives for both architectures are checksum verified, attested, and installed-artifact qualified. |
| Current native release | v0.7.1 | Supported on the four macOS/Linux targets listed above. |

## Product capabilities

| Capability | Rust CLI | Hosted hello.food | Status |
|---|---|---|---|
| Account connection | Bare `heyfood` | Account-neutral device authorization with hosted sign-in/create-account choice | Current source first-run flow |
| Registration | `heyfood register` | Explicit create-account device authorization, identity verification, agreements, and consent | Current source command |
| Login and authorization replacement | `heyfood login` | Fresh existing-account connection or atomic channel and application-session grant replacement | Current source command |
| Logout | `heyfood logout` | Refreshes expired authority before resolving and revoking the current channel link, device, and app session | Supported; no refresh or retry occurs after teardown begins, local account-bound credentials are always cleared, and interrupted two-store teardown or uncertain preflight recovery is resumable |
| Food questions | `heyfood ask` | Hosted agent turn | Current source command |
| Conversation continuation | `heyfood reply --conversation-id …` | Hosted conversation state | Current source command |
| Meal logging | `heyfood log` | Hosted agent and meal memory; direct use requires controlling-terminal `LOG` authorization | Current human-terminal-only source command |
| Item evaluation | `heyfood item` | Restaurant/menu evidence and dietary evaluation | Current source command |
| Grocery | `heyfood grocery` plus TUI confirmation cards | Read/export; direct preparation and confirm/cancel require exact controlling-terminal review phrases | Supported v0.7.1 command with human-only mutations |
| Oura health integrations | Not advertised; retained command spelling fails closed with `capability_deferred` | Future provider-neutral integration work | Deferred from the supported `v0.7.1` contract; no implementation or canary release gate |
| Apple Health | No CLI command or TUI panel | Mobile/backend work remains outside this release | Deferred from the supported `v0.7.1` contract |
| Native household roster and declared profiles | TUI `/household`, `/household add`, and `/onboard --for …` in `NativeEnabled` mode | Hosted member profile sync is deferred | Supported in v0.7.1: the human TUI adds or onboards active members atomically; complete version-1 declared profiles remain encrypted and local to this device |
| Household context switching | TUI `/for me`, `/for <member>`, and `/for everyone`; committed scope survives restart | Ordinary hosted turns use the exact selected local declared-profile snapshot | Supported in v0.7.1: Me/member/Everyone selection is persistent and drives request-first hosted evaluation without creating remote member profiles |
| Member dietary graph | Complete declared version-1 local profile plus hosted evaluation from that transient declared context | Learned history/preferences, goals, health/fitness, cross-device sync, remote member sync, and remote erasure are deferred | Partially supported in v0.7.1 with explicit limitations |
| Restaurants and recipes | Via `ask` and `item` | Resolution, menu evidence, and recipe tools | Hosted through current commands |
| Menu Watch | `heyfood watch` plus `/watch` TUI panel | List plus human-terminal-only create (`CREATE`) and remove (`REMOVE`); scheduled execution is deployed; latest account-owned change summary, source, freshness, and provenance render in the TUI | Current source management and summary view; item-level diff detail remains follow-on |
| Interactive TUI | Launches authenticated chat and functional read/action panels | Healthy slow work emits heartbeats; stage/tool identifiers remain outside response content | Supported in v0.7.1 with a responsive framed layout, branded startup view, bordered composer, contextual location and household scope, full command discovery, long-menu paging, and version/channel cues. Dietary setup uses responsive one-, two-, or three-column keyboard selectors with non-color selection markers, and `NO_COLOR` disables emitted color. Human output uses structured guidance, never serializes unsupported protocol JSON, and renews native authorization before each authenticated operation. |
| Voice capture | TUI `/voice`, Ctrl+Space, and F8 only in opt-in `native-audio` artifacts | Authenticated transcription | Not enabled in the default `0.7.1` build; real-hardware and platform qualification remain future work. |
| Agent self-description | `heyfood agent describe/guide/schema/doctor` | None | Supported in v0.7.1; offline, credential-free, bounded, and non-mutating. |
| Codex/Claude Agent Skill setup | `heyfood agent setup` / `agent uninstall` | N/A | Supported in v0.7.1 for the exact qualified host versions; dry-run is default, apply requires the reviewed plan digest, and setup registers MCP through host-owned commands. |
| Local read-only MCP | `heyfood mcp serve` | Existing authenticated read controllers | Supported in v0.7.1 with exactly six bounded read/discovery tools, bounded cursor pagination, and no agent mutation surface. |
| Agent-aware household Phase 0 | No public command or tool | Local contracts and fake-port proof only | Closed source prototype: v3 manifest, disclosure, local approval, native-state, and read/prepare/status/cancel contracts are frozen for exact-SHA review. The v0.7.0 manifest, six-tool MCP surface, installer, and public claims remain unchanged. |

## Process contract

`--json` emits exactly one ANSI-free JSON value on stdout. Human diagnostics
and progress use stderr. Redirected UTF-8 stdin is accepted by `ask`, `reply`,
`log`, and `item`. Runtime failures use nonzero exit status and machine-readable
errors; uncertain write outcomes are explicit.

See [CLI_CONTRACT.md](CLI_CONTRACT.md) for the stable process interface.
