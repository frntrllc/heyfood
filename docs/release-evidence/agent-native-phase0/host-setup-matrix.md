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

### Codex user scope

1. Dry-run resolves the exact `codex` executable and verifies the compatible
   version.
2. Plugin distribution prefers a reviewed marketplace followed by
   `codex plugin add heyfood@MARKETPLACE --json`.
3. Local stdio MCP uses the host-owned command:

   ```text
   codex mcp add heyfood -- /absolute/verified/path/heyfood mcp serve
   ```

4. Setup verifies the resulting entry with `codex mcp get heyfood` only after
   the user authorized apply.
5. Uninstall uses `codex mcp remove heyfood` and
   `codex plugin remove heyfood@MARKETPLACE --json`, conditional on the exact
   installation receipt.

The configured MCP command must be the verified absolute executable path.
Bare `heyfood` resolution through `PATH` is prohibited.
The host entry supplies no environment variables. MCP startup enforces the
frozen environment policy before credential access; a setup receipt records
its digest and the exact empty host environment.

### Codex project scope

Repository guidance and skill/plugin marketplace source may live under the
explicit project root. Project MCP configuration requires the trusted
project's `.codex/config.toml`; the observed `codex mcp add` command has no
project-scope flag and manages user configuration.

Therefore setup does not pretend a host-owned project-MCP command exists. It
must either:

- return `user_action_required` with an exact reviewed snippet; or
- after separate implementation review, use the plan's schema-aware,
  lock-protected, atomic, receipt-bound shared-file adapter.

It never silently appends to project configuration.

### Claude user scope

1. Dry-run resolves the exact `claude` executable and compatible version.
2. Plugin distribution uses a reviewed marketplace and
   `claude plugin install heyfood@MARKETPLACE --scope user`.
3. MCP uses:

   ```text
   claude mcp add --transport stdio --scope user heyfood -- /absolute/verified/path/heyfood mcp serve
   ```

4. Uninstall uses matching `--scope user` plugin and MCP removal.

The Claude entry also supplies no environment variables and is bound to the
same frozen MCP environment-policy digest.

### Claude project scope

Project setup requires an explicit existing absolute project root. Plugin
installation uses `--scope project`; MCP uses `--scope project` and therefore
lets Claude own `.claude/settings.json` and `.mcp.json` changes. Claude prompts
before using project-scoped MCP servers. Setup reports
`user_action_required` until that trust decision is completed.

Local scope remains user-private to one project and is never conflated with
project/team scope.

## Receipts and rollback

Every applied action records:

- target host, exact version, scope, and project root when applicable;
- absolute host and heyfood executable identities;
- marketplace/plugin identity and digest;
- MCP name, transport, exact command/arguments, and configuration owner;
- exact empty MCP environment plus the frozen environment-policy digest;
- expected prior state/digest;
- actions completed and outstanding user handoffs; and
- uninstall/rollback operation.

Receipts contain no credentials and use owner-only storage. Replacement and
uninstall require an exact matching prior receipt. If the current state has
changed, setup preserves it and reports a conflict.

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
