# Agent Skill setup

heyfood ships one canonical Agent Skill for qualified Codex and Claude Code
hosts. Setup is opt-in and never runs from the normal CLI installer.

## Qualified hosts

| Host | Qualified version | User skill path | Project skill path |
|---|---|---|---|
| Codex CLI | `codex-cli 0.145.0-alpha.18` | `~/.agents/skills/heyfood` | `<root>/.agents/skills/heyfood` |
| Claude Code | `2.1.128 (Claude Code)` | `~/.claude/skills/heyfood` | `<root>/.claude/skills/heyfood` |

Other versions fail closed until their installed-artifact behavior is
qualified. The setup plan reports the observed host executable and version.

## Preview and apply

Dry-run is the default and changes nothing:

```bash
heyfood --json agent setup --target all --scope user --dry-run
```

Apply the exact displayed plan:

```bash
heyfood --json agent setup --target all --scope user --apply
```

Project scope requires an explicit absolute Git worktree:

```bash
heyfood --json agent setup --target codex --scope project \
  --project-root /absolute/project --dry-run
```

Project setup installs only the host’s scoped skill. Claude Code may still ask
the user to trust that project. Setup does not answer or bypass host trust and
permission prompts.

## Update and uninstall

Re-running the same apply is idempotent. A binary or package upgrade reports a
conflict until the user supplies `--replace`; replacement is accepted only
when the installed files still match the prior receipt. Replace one host at a
time so the prior installation remains recoverable.

Preview removal:

```bash
heyfood --json agent uninstall --target all --scope user --dry-run
```

Then apply:

```bash
heyfood --json agent uninstall --target all --scope user --apply
```

Uninstall removes only an exact receipt-bound installation. Modified files,
unreceipted files, symlinks, reparse points, hard links, and unrelated host
configuration are preserved and reported as conflicts.

## Boundaries

- The skill teaches agents to prefer typed MCP reads when available and exact
  manifest-listed `agent_safe` JSON CLI routes otherwise.
- The skill never teaches an agent to drive the TUI or invoke human-only
  mutations.
- Setup never stores credentials or adds permissions.
- Health, native voice, Windows distribution, and agent mutations remain
  deferred unless the exact installed manifest says otherwise.
- MCP configuration is not installed until the separately qualified read-only
  MCP phase is active.
