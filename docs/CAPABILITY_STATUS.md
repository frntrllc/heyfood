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
| Native v0.7.1 | Previous supported release | Redesigned the interactive terminal with a calm responsive frame, branded startup view, bordered composer, and contextual working, location, household-scope, version, and release-channel cues. Superseded by v0.8.0. |
| Native v0.8.0 | Previous supported release | Added manifest-v3 discovery and bounded, explicitly authorized read-only agent understanding of the encrypted local household roster and additional-member profiles. Superseded by v0.9.0. |
| Native v0.9.0 | Current supported release | Adds a capability-gated, read-only Diet guide across the CLI, TUI, and local MCP while retaining the v0.8.0 household boundary. Agent mutations remain unavailable. |
| Hosted installer | Supported | Installs the checksum-verified native `v0.9.0` archive for macOS or Linux. |
| Source | Available | Public for inspection and contributor evaluation under Apache 2.0. |
| Windows x86-64 | Deferred | Windows distribution and Windows CI are outside the v0.9.0 release train. The retained Windows source is not a support claim. |
| v0.9.0 release asset set | Supported | Exactly four product archives, four matching standalone-verifier archives, one canonical native-state declaration, and `SHA256SUMS`; every asset is attested and verified again after public download. |
| macOS v0.9.0 archives | Supported | The product and verifier executables for both architectures are Developer ID signed with hardened runtime, Apple notarized, attested, and installed-artifact qualified. |
| Linux v0.9.0 archives | Supported | The product and verifier archives for both architectures are checksum verified, attested, and installed-artifact qualified. |
| Current native release | v0.9.0 | Supported on the four macOS/Linux targets listed above. |

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
| Diet guide | `heyfood diet`, `diet list`, `diet show DIET_ID`, short-form `diet DIET_ID`, plus TUI `/diet [DIET_ID]` | 22 runtime-served Diet v1 guides with authored ordering and evidence levels | Supported read-only v0.9.0 capability; requires exact `diet:v1` discovery and `knowledge:read`. Runtime IDs are case-sensitive. Profile set/clear is deferred pending [hellofood #261](https://github.com/frntrllc/hellofood/issues/261). |
| Grocery | `heyfood grocery` plus TUI confirmation cards | Read/export; direct preparation and confirm/cancel require exact controlling-terminal review phrases | Supported v0.9.0 command with human-only mutations |
| Oura health integrations | Not advertised; retained command spelling fails closed with `capability_deferred` | Future provider-neutral integration work | Deferred from the supported `v0.9.0` contract; no implementation or canary release gate |
| Apple Health | No CLI command or TUI panel | Mobile/backend work remains outside this release | Deferred from the supported `v0.9.0` contract |
| Native household roster and declared profiles | Human TUI `/household`, `/household add`, and `/onboard --for …`; agents may use the discovered read-only roster and exact additional-member profile commands/tools after per-subject disclosure | Hosted member profile sync is deferred | Supported in v0.9.0: complete version-1 declared profiles remain encrypted locally; the agent surface is minimized, read-only, and capability-gated |
| Household context switching | Human TUI `/for me`, `/for <member>`, and `/for everyone`; committed scope survives restart | Ordinary hosted turns use the exact selected local declared-profile snapshot | Supported in v0.9.0 for humans; the agent handoff is limited to one exact disclosed additional member and cannot change scope |
| Member dietary graph | Complete declared version-1 local profile plus hosted evaluation from that transient declared context | Learned history/preferences, goals, health/fitness, cross-device sync, remote member sync, and remote erasure are deferred | Partially supported in v0.9.0 with explicit limitations |
| Restaurants and recipes | Via `ask` and `item` | Resolution, menu evidence, and recipe tools | Hosted through current commands |
| Menu Watch | `heyfood watch` plus `/watch` TUI panel | List plus human-terminal-only create (`CREATE`) and remove (`REMOVE`); scheduled execution is deployed; latest account-owned change summary, source, freshness, and provenance render in the TUI | Current source management and summary view; item-level diff detail remains follow-on |
| Interactive TUI | Launches authenticated chat and functional read/action panels | Healthy slow work emits heartbeats; stage/tool identifiers remain outside response content | Supported in v0.9.0 with the responsive framed layout, branded startup view, bordered composer, contextual location and household scope, full command discovery, long-menu paging, and version/channel cues introduced in v0.7.1. Dietary setup uses responsive one-, two-, or three-column keyboard selectors with non-color selection markers, and `NO_COLOR` disables emitted color. |
| Voice capture | TUI `/voice`, Ctrl+Space, and F8 only in opt-in `native-audio` artifacts | Authenticated transcription | Not enabled in the default `0.9.0` build; real-hardware and platform qualification remain future work. |
| Agent self-description | `heyfood agent describe/guide/schema/doctor/compatibility` | None | Supported in v0.9.0; offline, credential-free, bounded, and non-mutating. Explicit schema-v1 through schema-v3 views remain frozen; schema v4 is the default. |
| Codex/Claude Agent Skill setup | `heyfood agent setup` / `agent uninstall` | N/A | Supported in v0.9.0 for the exact qualified host versions; dry-run is default, apply requires the reviewed plan digest, and setup registers MCP through host-owned commands. |
| Local read-only MCP | `heyfood mcp serve` | Existing authenticated read controllers plus account-bound local household and Diet reads | Supported in v0.9.0 with ten tools: the original six, two disclosure-gated household reads, and two capability-gated Diet reads. Pagination is bounded and no agent mutation surface exists. |
| Agent household mutations | No command or MCP tool | None | Deferred. Add, edit, archive, restore, persistent scope changes, proposal preparation, approval, commit, and TUI automation remain human-only or absent. |

Diet guides and the dietary onboarding catalog are related but distinct. The
service currently returns 22 authored guides, while the profile-writable
catalog contains 26 selectable diet options. A missing guide is not permission
to invent an explanation: `diet_not_covered` is rendered as a successful,
truthful result. Evidence levels (`strong`, `moderate`, or `limited`) describe
source support and are advisory rather than clinical or safety guarantees.
Safety remains visible on evaluated food, and optional `diet_alignment` is
strictly subordinate: it never changes a safety badge, color, order, filter,
or status and never mutates the profile.

## Process contract

`--json` emits exactly one ANSI-free JSON value on stdout. Human diagnostics
and progress use stderr. Redirected UTF-8 stdin is accepted by `ask`, `reply`,
`log`, and `item`. Runtime failures use nonzero exit status and machine-readable
errors; uncertain write outcomes are explicit.

See [CLI_CONTRACT.md](CLI_CONTRACT.md) for the stable process interface.
