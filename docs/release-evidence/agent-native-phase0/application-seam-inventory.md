# Agent-native Phase 0 application seam inventory

**Baseline:** `d68091a9cf6341c2c9120ba9251a6e0dd79a9616`  
**Status:** extraction in progress

| Workflow | Current application seam | Concrete orchestration still outside application | Phase 0 disposition |
|---|---|---|---|
| Conversation | `ServicePort`, `RunTurn`, `execute_one_shot_turn` | Prompt shaping, household context, rendering, and interactive continuity remain in `heyfood-bin` | Preserve existing use case; extract context/controller ownership without changing fixtures |
| Session refresh | `EnsureSession`, `CredentialPort` | Composition and scope routing remain in `heyfood-bin` | Retain; expose only through composed controllers |
| Grocery | Deployed `GroceryReadPort`, renderer-neutral display/exclusion snapshots, and production read controllers; provisional authority-bearing `GroceryPort` plus internal `ReadActiveGroceryList` proof | Prepare/confirm still call concrete `HttpService`; the deployed active-list response cannot satisfy the authority-bearing port | Display read extraction complete; keep mutation authority separate until the backend supplies the frozen context fingerprint |
| Menu Watch | `MenuWatchPort`; renderer-neutral snapshots and create request; list/create/remove controllers; production `HttpService` adapter | Argument parsing and human/JSON rendering remain in CLI/bin | Controller extraction complete; direct create/remove now require distinct controlling-terminal review before credentials or network |
| Capability/status | `CapabilityPort`, renderer-neutral `CapabilitySnapshot`, and `DiscoverCapabilities`; `HttpService` implements the production adapter | Scope interpretation, profile/status composition, voice readiness, and panel text remain in the binary | Discovery extraction complete; define the composed status controller without moving rendering into application |
| Household/profile context | `TurnContext` only | Imported-state parsing and profile downloads are in binary | Extract only the context assembly needed by shared workflows |
| Registration/login | Registration runtime plus binary orchestration | Browser/device handoff and durable replacement remain composition-owned | Inventory only; agent setup must not redefine auth |
| Health | Provider-neutral types/port retained | Hidden runtime paths exist but public dispatch fails closed | Keep deferred and absent from agent tools |

## Dependency findings

- `heyfood-application` does not depend on runtime, platform, CLI, TUI, or bin.
- `heyfood-agent-runtime` implements `ServicePort`, `CapabilityPort`,
  `GroceryReadPort`, and `MenuWatchPort`; Grocery prepare/confirm operations
  remain inherent `HttpService` methods.
- `heyfood-bin::OneShotExecutor` still accepts `&HttpService` for Grocery
  prepare/confirm, preventing a fake authority-bearing Grocery service without
  exercising the concrete runtime type.
- `InteractiveTurnDriver` has an object-safe conversational service and a
  second optional concrete `HttpService` specifically for panels and other
  workflows.
- One-shot Grocery dispatch plus the interactive Status and Grocery panels now
  discover service capabilities through `DiscoverCapabilities`; the existing
  renderer and operation behavior remain unchanged.
- One-shot Grocery list/exclusion reads, item-reference refreshes, and the
  interactive Grocery panel route through the deployed display-read
  controllers. Runtime conversion preserves the frozen JSON shape, including
  member safety, substitutions, and provenance.
- One-shot Menu Watch read/create/remove and the interactive Watch panel now
  route through the same application controllers. The application snapshots
  preserve the frozen JSON representation, and the existing binary route test
  still exercises all Watch endpoints.
- Direct meal log, Grocery proposal/confirmation, and Menu Watch mutation paths
  open the controlling terminal separately from stdin/stdout and require exact
  command-specific phrases before credential access. Public-binary negative
  tests cover all classified routes and prove missing-terminal failure opens no
  socket; positive exact-artifact PTY and Windows console qualification remain.
- The deployed active-list REST shape does not contain the context fingerprint
  required by provisional `GroceryListSnapshot::preconditions`. A production
  `GroceryPort` adapter cannot invent it. Phase 0 must either separate
  renderer-neutral list reads from mutation-authority snapshots or obtain an
  authoritative backend shape; it must not weaken the frozen preconditions.

The internal authority-bearing Grocery fake-port controller proves object-safe
dependency direction and cancellation only. The deployed display-read seam
has a production HTTP adapter, exact JSON mapping evidence, and pre-dispatch
cancellation tests. Capability discovery also has a production adapter and
both cancellation and forwarding tests. The next Grocery increment must obtain
or freeze the real mutation-authority shape; it must not infer authority from
display data.
