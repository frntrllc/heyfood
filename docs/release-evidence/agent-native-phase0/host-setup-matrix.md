# Agent-native Phase 0 host setup matrix

**Observed:** 2026-07-27  
**Status:** exact development-host matrix frozen; public compatibility remains
subject to Phase 5 installed-artifact qualification

## Source authority

- Codex discovers repository `AGENTS.md`, standalone `SKILL.md` skills,
  plugins, and MCP servers through distinct supported mechanisms. Repository
  skills live under `.agents/skills`; user skills live under
  `~/.agents/skills`. [Codex Agent Skills](https://learn.chatgpt.com/docs/build-skills)
- Codex plugins are installable packages and local/repository marketplaces are
  supported development and team-distribution sources.
  [`codex plugin` reference](https://learn.chatgpt.com/docs/developer-commands?surface=cli#cli-codex-plugin)
- Codex supports `codex mcp add/remove/list/get` for user configuration and
  project-scoped `.codex/config.toml` in trusted repositories.
  [Codex MCP configuration](https://learn.chatgpt.com/docs/mcp)
- Claude Code reads `CLAUDE.md`, supports `@AGENTS.md` imports, and recommends
  the import when a repository already uses `AGENTS.md`.
  [Claude Code memory](https://code.claude.com/docs/en/memory)
- Claude Code plugins may bundle skills and MCP definitions; user, project,
  and local installation scopes are explicit.
  [Claude plugin installation](https://code.claude.com/docs/en/discover-plugins)
- Claude MCP supports local, project, and user scopes through
  `claude mcp add/remove`.
  [Claude MCP scopes](https://code.claude.com/docs/en/mcp)

## Exact development hosts

| Host | Exact observed version | Verified local management surfaces | Phase 0 disposition |
|---|---|---|---|
| Codex CLI bundled with ChatGPT desktop | `codex-cli 0.145.0-alpha.18` | `codex plugin add/remove/list/marketplace`; `codex mcp add/remove/get/list/login/logout` | Eligible for private Phase 2/3 candidate tests only |
| Claude Code | `2.1.128` | `claude plugin install/uninstall/enable/disable/marketplace`; `claude mcp add/remove/get/list/reset-project-choices` | Eligible for private Phase 2/3 candidate tests only |

The checked commands were help/version reads only. No plugin, marketplace, MCP
server, skill, instruction, or configuration was installed or modified.

## Frozen setup behavior

The executable setup path and separately downloadable plugin packages are
distinct mechanisms. `heyfood agent setup` installs the canonical six-file
standalone skill and registers the local MCP server; it does not claim that a
marketplace plugin was installed.

Every apply is bound to the SHA-256 of a complete dry-run plan. Setup rechecks
the plan after acquiring its owner-only lock and stops if any host, file,
binary, registration, or receipt identity changed.

### Codex user scope

1. Dry-run resolves the exact `codex` executable, verifies the qualified
   version, inspects `codex mcp get heyfood --json`, and plans the standalone
   skill at `~/.agents/skills/heyfood`.
2. Apply installs that skill and invokes the host-owned command:

   ```text
   codex mcp add heyfood -- /absolute/verified/path/heyfood mcp serve
   ```

3. Setup verifies the resulting JSON entry: stdio transport, the exact
   executable, exact `mcp serve` arguments, and no environment entries.
4. Receipt-bound uninstall invokes `codex mcp remove heyfood`, verifies
   absence, and removes only the exact installed skill.

### Codex project scope

The observed Codex `mcp add` command has no project-scope flag. Project scope
therefore fails closed with a concrete conflict and directs the user to the
qualified user-scope setup. Setup never edits `.codex/config.toml` itself and
never represents a user-level registration as project-local.

### Claude user scope

1. Dry-run verifies the exact host and inspects the named entry.
2. Apply installs `~/.claude/skills/heyfood` and invokes:

   ```text
   claude mcp add --transport stdio --scope user heyfood -- /absolute/verified/path/heyfood mcp serve
   ```

3. Setup verifies the reported user scope, stdio transport, exact command and
   arguments, and empty environment.
4. Uninstall uses `claude mcp remove --scope user heyfood`.

### Claude project scope

Project setup requires an explicit, absolute, existing Git worktree. The skill
is installed at `.claude/skills/heyfood`; MCP registration is delegated to:

```text
claude mcp add --transport stdio --scope project heyfood -- /absolute/verified/path/heyfood mcp serve
```

Claude owns its project configuration and normal trust prompt. The plan
reports that trust decision as a user action; setup does not click or bypass
it.

For all scopes, the configured command is the verified absolute executable,
not a bare `heyfood` resolved through `PATH`. Registration supplies no
environment entries. MCP startup independently rejects every inherited
`HEYFOOD_*` value before credential access, network dispatch, or protocol
stdout.

## Receipts and rollback

Every applied action records:

- target host, exact version, scope, and project root when applicable;
- absolute host and heyfood executable identities;
- canonical standalone-skill package identity and every installed file digest;
- MCP name, stdio transport, exact command/arguments, and configuration scope;
- exact empty MCP environment plus the frozen environment-policy digest;
- and the exact receipt-bound skill destination.

Receipts contain no credentials and use owner-only storage. Replacement and
uninstall require an exact matching prior receipt. If the current state has
changed, setup preserves it and reports a conflict. Multi-host uninstall
stages all receipt and skill removals before host changes; failures restore
the staged set or return an explicit uncertain outcome. Link/reparse
substitution is blocked by anchored no-follow directory handles.

## Unsupported combinations

- ChatGPT web cannot run the local stdio heyfood server and is not part of this
  local-host matrix.
- A Codex or Claude version other than the exact versions above is unqualified
  until the Phase 5 matrix adds it.
- Project setup without an explicit absolute project root is rejected.
- Managed/admin policy denial is reported; setup never bypasses it.
- Required UI clicks or trust prompts return `user_action_required`; they are
  not reported as completed.
- Direct edits to general `AGENTS.md` or `CLAUDE.md` are not an installation
  strategy.
