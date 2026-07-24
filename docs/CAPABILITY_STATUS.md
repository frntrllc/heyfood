# Capability and distribution status

This page is the authoritative public boundary between the current Rust command
surface, hosted hello.food capabilities, and preview work. Recognition of a
hidden legacy command is not support: unavailable paths return
`command_not_available`.

## Distribution

| Surface | Status | Meaning |
|---|---|---|
| Native v0.4.0 | Unsupported | Do not install or use. Published before release authorization. |
| Native v0.4.1 | Unsupported | Do not install or use. Published before release authorization. |
| Hosted installer | Suspended | Prints the incident notice and exits `1` without installing anything. |
| Source | Available | Public for inspection and contributor evaluation under Apache 2.0. |
| Windows x86-64 archive | Source/CI qualification | Deterministic zip packaging, installed-executable smoke, and mandatory release Authenticode verification are wired; protected signing credentials, first signed execution, and the public per-user installer remain release blockers. |
| macOS native archives | Source/CI qualification | Mandatory Developer ID hardened-runtime signing and Apple notarization are wired into the protected release environment; protected credentials and first notarized execution remain release blockers. |
| Replacement native release | Unavailable | Not available until testing and release approval are complete. |

## Product capabilities

| Capability | Rust CLI | Hosted hello.food | Status |
|---|---|---|---|
| Registration | `heyfood register` | Device authorization, identity verification, agreements, and consent | Current source command |
| Login and scope upgrade | `heyfood login` | Atomic channel and application-session grant | Current source command |
| Food questions | `heyfood ask` | Hosted agent turn | Current source command |
| Conversation continuation | `heyfood reply --conversation-id …` | Hosted conversation state | Current source command |
| Meal logging | `heyfood log` | Hosted agent and meal memory | Current source command |
| Item evaluation | `heyfood item` | Restaurant/menu evidence and dietary evaluation | Current source command |
| Grocery | `heyfood grocery` plus TUI confirmation cards | Read, prepare, export, explicitly confirm/cancel, and correct pending add-item names | Current source command; production canaries pending |
| Oura health context | `heyfood health` | Connect, sync, read, and disconnect | Current source command |
| Apple Health | No direct CLI command | Daily summaries arrive through the hello.food app | Backend available |
| Household context | Used by hosted turns and Grocery | Profiles and household-aware evaluation | Backend available; native roster management unavailable |
| Restaurants and recipes | Via `ask` and `item` | Resolution, menu evidence, and recipe tools | Hosted through current commands |
| Menu Watch | `heyfood watch` plus `/watch` TUI panel | Create/list/remove are deployed; scheduled execution remains operationally gated; no account-scoped diff-read route exists | Current source management command; diff view blocked on backend contract |
| Interactive TUI | Draft branch launches authenticated chat and read panels | N/A | Source preview; packaged archives run the bounded `0.5.0` clean/returning-user, household Grocery, failure-safety, and 40/80/120-column matrix in CI. Signed-candidate reruns and production canaries remain pending; the broader landing-page journeys are future conformance work. |
| Voice capture | TUI `/voice`, Ctrl+Space, and F8 only in opt-in `native-audio` artifacts | Authenticated transcription | Not enabled in the default `0.5.0` build and not a recovery-release gate; real-hardware and platform qualification remain future work. |

## Process contract

`--json` emits exactly one ANSI-free JSON value on stdout. Human diagnostics
and progress use stderr. Redirected UTF-8 stdin is accepted by `ask`, `reply`,
`log`, and `item`. Runtime failures use nonzero exit status and machine-readable
errors; uncertain write outcomes are explicit.

See [CLI_CONTRACT.md](CLI_CONTRACT.md) for the stable process interface.
