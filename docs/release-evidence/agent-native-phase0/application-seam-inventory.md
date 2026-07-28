# Agent-native Phase 0 application seam inventory

**Baseline:** `d68091a9cf6341c2c9120ba9251a6e0dd79a9616`  
**Status:** extraction in progress

| Workflow | Current application seam | Concrete orchestration still outside application | Phase 0 disposition |
|---|---|---|---|
| Conversation | `ServicePort`, `RunTurn`, `execute_one_shot_turn` | Prompt shaping, household context, rendering, and interactive continuity remain in `heyfood-bin` | Preserve existing use case; extract context/controller ownership without changing fixtures |
| Session refresh | `EnsureSession`, `CredentialPort` | Composition and scope routing remain in `heyfood-bin` | Retain; expose only through composed controllers |
| Grocery | Deployed read, export, and mutation ports with shared controllers; exact display DTOs plus opaque server-signed proposal/confirmation DTOs; provisional stronger `GroceryPort` remains separate | Argument parsing, controlling-terminal ceremony, proposal stdin, export persistence, and human/JSON rendering remain in CLI/bin | Current CLI extraction complete; do not activate the stronger provisional port until the backend supplies its frozen context fingerprint |
| Menu Watch | `MenuWatchPort`; renderer-neutral snapshots and create request; list/create/remove controllers; production `HttpService` adapter | Argument parsing and human/JSON rendering remain in CLI/bin | Controller extraction complete; direct create/remove now require distinct controlling-terminal review before credentials or network |
| Capability/status | `CapabilityPort`, `StatusPort`, renderer-neutral capability/status snapshots, `DiscoverCapabilities`, and `ReadStatus`; `HttpService` implements both production adapters | Human panel text remains in the binary | Controller extraction complete; rendering remains presentation-owned |
| Household/profile context | `TurnContext`, `ServicePort`, and shared turn controllers | Imported-state parsing, profile downloads, and presentation are composition-owned | Existing conversational application seam is sufficient for Phase 0; later agent tools must reuse it |
| Registration/login | Registration runtime plus binary orchestration | Browser/device handoff and durable replacement remain composition-owned | Inventory only; agent setup must not redefine auth |
| Health | Provider-neutral types/port retained | Hidden runtime paths exist but public dispatch fails closed | Keep deferred and absent from agent tools |

## Dependency findings

- `heyfood-application` does not depend on runtime, platform, CLI, TUI, or bin.
- `heyfood-agent-runtime` implements `ServicePort`, `CapabilityPort`,
  `StatusPort`, `GroceryReadPort`, `GroceryExportPort`,
  `GroceryMutationPort`, and `MenuWatchPort`.
- `heyfood-bin::OneShotExecutor` remains the composition owner of the concrete
  `HttpService`, but each in-scope workflow dispatches through an object-safe
  application port/controller rather than calling a runtime operation
  directly.
- `InteractiveTurnDriver` has an object-safe conversational service and a
  second optional concrete `HttpService` specifically for panels and other
  workflows.
- One-shot Grocery dispatch plus the interactive Grocery panel discover service
  capabilities through `DiscoverCapabilities`. The interactive Status panel
  now uses `ReadStatus`, which composes capability discovery, authorization
  scopes, profile consent, and local voice readiness into a typed snapshot.
  Existing renderer and operation behavior remain unchanged.
- One-shot Grocery list/exclusion reads, item-reference refreshes, and the
  interactive Grocery panel route through the deployed display-read
  controllers. Runtime conversion preserves the frozen JSON shape, including
  member safety, substitutions, and provenance.
- One-shot Grocery prepare, confirmation, and export route through dedicated
  application controllers and production adapters. The human CLI retains its
  exact server proposal/confirmation wire and opaque signed token; no model or
  agent surface is added.
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
  `GroceryPort` adapter cannot invent it. The stronger provisional port stays
  inactive and separate from the exact deployed human-CLI port; later work
  must obtain an authoritative backend shape before using that stronger
  contract.

The internal authority-bearing Grocery fake-port controller proves object-safe
dependency direction and cancellation only. The deployed display-read seam
has a production HTTP adapter, exact JSON mapping evidence, and pre-dispatch
cancellation tests. Deployed Grocery proposal/confirmation and export also
have application controllers, production adapters, pre-dispatch cancellation
tests, and existing public-binary route coverage. Capability and composed
status discovery have production adapters plus cancellation and composition
tests.
