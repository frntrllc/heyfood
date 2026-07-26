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
| Hosted installer | Supported | Installs the checksum-verified native `v0.5.0` archive for macOS or Linux. |
| Source | Available | Public for inspection and contributor evaluation under Apache 2.0. |
| Windows x86-64 | Deferred | Windows distribution is deferred to `v0.5.1`. Ordinary Windows compile, test, Clippy, Credential Manager, and deterministic packaging qualification remains active in CI, but no Windows credential or asset participates in the `v0.5.0` release path. |
| macOS native archives | Supported in v0.5.0 | Both architectures are Developer ID signed with hardened runtime, Apple notarized, and installed-artifact qualified. |
| Linux native archives | Supported in v0.5.0 | Both architectures are checksum verified, attested, and installed-artifact qualified. |
| Replacement native release | v0.5.0 | Supported on the four macOS/Linux targets listed above. |

## Product capabilities

| Capability | Rust CLI | Hosted hello.food | Status |
|---|---|---|---|
| Registration | `heyfood register` | Device authorization, identity verification, agreements, and consent | Current source command |
| Login and authorization replacement | `heyfood login` | Atomic channel and application-session grant | Current source command |
| Food questions | `heyfood ask` | Hosted agent turn | Current source command |
| Conversation continuation | `heyfood reply --conversation-id …` | Hosted conversation state | Current source command |
| Meal logging | `heyfood log` | Hosted agent and meal memory | Current source command |
| Item evaluation | `heyfood item` | Restaurant/menu evidence and dietary evaluation | Current source command |
| Grocery | `heyfood grocery` plus TUI confirmation cards | Read, prepare, export, explicitly confirm/cancel, and correct pending add-item names | Supported v0.5.0 command |
| Oura health integrations | Not advertised; retained command spelling fails closed with `capability_deferred` | Future provider-neutral integration work | Deferred from the supported `v0.5.0` contract; no implementation or canary release gate |
| Apple Health | No CLI command or TUI panel | Mobile/backend work remains outside this release | Deferred from the supported `v0.5.0` contract |
| Household context | Used by hosted turns and Grocery | Profiles and household-aware evaluation | Backend available; native roster management unavailable |
| Restaurants and recipes | Via `ask` and `item` | Resolution, menu evidence, and recipe tools | Hosted through current commands |
| Menu Watch | `heyfood watch` plus `/watch` TUI panel | Create/list/remove are deployed; scheduled execution remains operationally gated; no account-scoped diff-read route exists | Current source management command; diff view blocked on backend contract |
| Interactive TUI | Launches authenticated chat and functional read/action panels | N/A | Supported in v0.5.0; packaged archives pass the bounded clean/returning-user, household Grocery, failure-safety, and 40/80/120-column matrix. |
| Voice capture | TUI `/voice`, Ctrl+Space, and F8 only in opt-in `native-audio` artifacts | Authenticated transcription | Not enabled in the default `0.5.0` build and not a recovery-release gate; real-hardware and platform qualification remain future work. |

## Process contract

`--json` emits exactly one ANSI-free JSON value on stdout. Human diagnostics
and progress use stderr. Redirected UTF-8 stdin is accepted by `ask`, `reply`,
`log`, and `item`. Runtime failures use nonzero exit status and machine-readable
errors; uncertain write outcomes are explicit.

See [CLI_CONTRACT.md](CLI_CONTRACT.md) for the stable process interface.
