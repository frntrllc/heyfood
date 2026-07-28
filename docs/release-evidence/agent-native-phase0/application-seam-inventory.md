# Agent-native Phase 0 application seam inventory

**Baseline:** `d68091a9cf6341c2c9120ba9251a6e0dd79a9616`  
**Status:** extraction in progress

| Workflow | Current application seam | Concrete orchestration still outside application | Phase 0 disposition |
|---|---|---|---|
| Conversation | `ServicePort`, `RunTurn`, `execute_one_shot_turn` | Prompt shaping, household context, rendering, and interactive continuity remain in `heyfood-bin` | Preserve existing use case; extract context/controller ownership without changing fixtures |
| Session refresh | `EnsureSession`, `CredentialPort` | Composition and scope routing remain in `heyfood-bin` | Retain; expose only through composed controllers |
| Grocery | `GroceryPort` plus internal `ReadActiveGroceryList` composition proof | `OneShotExecutor` calls concrete `HttpService`; runtime does not implement `GroceryPort`; TUI panels also call concrete service | Reconcile the provisional port with the deployed REST shapes, then add the production adapter and migrate CLI/TUI with parity |
| Menu Watch | None | Read/create/delete, validation, rendering selection, and panel loading are in binary/runtime | Define renderer-neutral port/controller before MCP |
| Capability/status | None | Capability discovery, scope interpretation, profile/status composition, voice readiness, and panel text are in binary | Define typed snapshot/controller; keep rendering in CLI/TUI |
| Household/profile context | `TurnContext` only | Imported-state parsing and profile downloads are in binary | Extract only the context assembly needed by shared workflows |
| Registration/login | Registration runtime plus binary orchestration | Browser/device handoff and durable replacement remain composition-owned | Inventory only; agent setup must not redefine auth |
| Health | Provider-neutral types/port retained | Hidden runtime paths exist but public dispatch fails closed | Keep deferred and absent from agent tools |

## Dependency findings

- `heyfood-application` does not depend on runtime, platform, CLI, TUI, or bin.
- `heyfood-agent-runtime` implements `ServicePort` but exposes Grocery and Menu
  Watch as inherent `HttpService` methods.
- `heyfood-bin::OneShotExecutor` accepts `&HttpService`, preventing a fake
  Grocery/Menu Watch service without exercising the concrete runtime type.
- `InteractiveTurnDriver` has an object-safe conversational service and a
  second optional concrete `HttpService` specifically for panels and other
  workflows.
- The deployed active-list REST shape does not contain the context fingerprint
  required by provisional `GroceryListSnapshot::preconditions`. A production
  `GroceryPort` adapter cannot invent it. Phase 0 must either separate
  renderer-neutral list reads from mutation-authority snapshots or obtain an
  authoritative backend shape; it must not weaken the frozen preconditions.

The internal fake-port controller proves object-safe dependency direction and
cancellation only. The next code increments must resolve the real Grocery
shape and remove concrete paths one workflow at a time, proving existing
CLI/TUI bytes and cancellation semantics before MCP depends on the extracted
controller.
