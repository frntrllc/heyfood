# Authentication and capabilities

Credentials stay inside heyfood's native credential backend. MCP and agent
commands never return them.

When authentication is missing:

1. Preserve the structured error code and user-facing handoff.
2. Ask the user to run the exact login or registration command named by the
   result.
3. Never ask the user to paste tokens, API keys, device codes, or refresh
   material into chat.
4. Retry only after the user reports completion and the status/capability read
   confirms readiness.

When scopes are missing, present the exact required scopes and authorization
upgrade command. OAuth refresh cannot widen authority.

Treat these states distinctly:

- unauthenticated;
- authenticated but insufficient scope;
- capability not advertised;
- capability deferred by the client;
- service unreachable; and
- uncertain credential rotation or reconciliation.

Never guess one into another.
