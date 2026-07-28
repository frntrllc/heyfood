//! Credential-free, network-free agent self-description dispatcher.

use std::fmt::Write as _;
use std::process::ExitCode;

use heyfood_agent_setup::{
    Host, SetupMode, SetupOperation, SetupOptions, SetupPlan, SetupScope, SetupTarget,
};
use heyfood_cli::{
    AgentCommand, AgentSetupArgs, AgentSetupScope, AgentSetupTarget, AgentUninstallArgs,
};
use heyfood_core::terminal_safe_text;
use serde_json::Value;

/// Run one local agent-discovery command before any credential or network setup.
#[must_use]
pub fn run(command: Option<AgentCommand>, machine: bool) -> ExitCode {
    match command.unwrap_or(AgentCommand::Describe) {
        AgentCommand::Describe => write_json(&heyfood_agent_contract::manifest()),
        AgentCommand::Guide(arguments) => {
            if machine {
                let document = if arguments.safety {
                    heyfood_agent_contract::safety_document()
                } else {
                    heyfood_agent_contract::guide_document()
                };
                write_json(&document);
            } else if arguments.safety {
                print!("{}", heyfood_agent_contract::SAFETY);
            } else {
                print!("{}", heyfood_agent_contract::GUIDE);
            }
        }
        AgentCommand::Schema(arguments) if arguments.list => {
            write_json(&heyfood_agent_contract::schema_index());
        }
        AgentCommand::Schema(arguments) => {
            let Some(name) = arguments.schema.as_deref() else {
                return failure(
                    machine,
                    "agent_schema_required",
                    "A public schema name or identifier is required.",
                    "Run `heyfood agent schema --list` to inspect supported schemas.",
                );
            };
            let Some(schema) = heyfood_agent_contract::schema_by_name(name) else {
                return failure(
                    machine,
                    "agent_schema_unknown",
                    "The requested schema is not part of the public installed contract.",
                    "Run `heyfood agent schema --list` and use an exact name or identifier.",
                );
            };
            print!("{}", schema.document());
        }
        AgentCommand::Doctor => write_json(&heyfood_agent_contract::doctor_document()),
        AgentCommand::Setup(arguments) => return setup(arguments, machine),
        AgentCommand::Uninstall(arguments) => return uninstall(arguments, machine),
    }
    ExitCode::SUCCESS
}

fn setup(arguments: AgentSetupArgs, machine: bool) -> ExitCode {
    run_setup(
        SetupOptions {
            target: map_target(arguments.target),
            scope: map_scope(arguments.scope),
            project_root: arguments.project_root,
            operation: SetupOperation::Install,
            mode: if arguments.apply {
                SetupMode::Apply
            } else {
                SetupMode::DryRun
            },
            replace: arguments.replace,
            expected_plan_sha256: arguments.plan_sha256,
        },
        machine,
    )
}

fn uninstall(arguments: AgentUninstallArgs, machine: bool) -> ExitCode {
    run_setup(
        SetupOptions {
            target: map_target(arguments.target),
            scope: map_scope(arguments.scope),
            project_root: arguments.project_root,
            operation: SetupOperation::Uninstall,
            mode: if arguments.apply {
                SetupMode::Apply
            } else {
                SetupMode::DryRun
            },
            replace: false,
            expected_plan_sha256: arguments.plan_sha256,
        },
        machine,
    )
}

fn run_setup(options: SetupOptions, machine: bool) -> ExitCode {
    match heyfood_agent_setup::execute(&options) {
        Ok(plan) => {
            if machine {
                let document = serde_json::to_value(plan).expect("setup plan is serializable");
                write_json(&document);
            } else {
                print!("{}", render_setup_plan(&plan));
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            let rendered = heyfood_cli::render_error_with_outcome(
                error.kind,
                &error.message,
                error.hint.as_deref(),
                machine,
                error.uncertain,
            )
            .expect("the stable error envelope is serializable");
            if machine {
                print!("{rendered}");
            } else {
                eprint!("{rendered}");
            }
            ExitCode::FAILURE
        }
    }
}

fn render_setup_plan(plan: &SetupPlan) -> String {
    let mut output = String::new();
    let operation = match plan.operation {
        SetupOperation::Install => "Agent integration setup",
        SetupOperation::Uninstall => "Agent integration removal",
    };
    let mode = match plan.mode {
        SetupMode::DryRun => "dry run",
        SetupMode::Apply => "applied",
    };
    let readiness = if plan.ready { "ready" } else { "blocked" };
    let _ = writeln!(output, "{operation} ({mode}): {readiness}");
    let _ = writeln!(
        output,
        "heyfood {} · package {} · {} files",
        terminal_safe_text(plan.binary.version),
        terminal_safe_text(plan.package.version),
        plan.package.files
    );

    for host in &plan.hosts {
        let host_name = match host.host {
            Host::Codex => "Codex",
            Host::Claude => "Claude Code",
        };
        let path = terminal_safe_text(&host.skill_path.display().to_string());
        let _ = writeln!(
            output,
            "\n{host_name}: {} · {}",
            terminal_safe_text(host.action),
            terminal_safe_text(host.compatibility)
        );
        let _ = writeln!(output, "  Skill: {path}");
        let _ = writeln!(
            output,
            "  MCP: {} · {} · {} mcp serve · environment empty",
            terminal_safe_text(host.mcp.action),
            terminal_safe_text(host.mcp.configuration_scope),
            terminal_safe_text(&host.mcp.command.display().to_string()),
        );
        let _ = writeln!(
            output,
            "  MCP environment policy: {}",
            terminal_safe_text(&host.mcp.environment_policy_sha256)
        );
        if let Some(version) = host.host_version.as_deref() {
            let _ = writeln!(output, "  Host: {}", terminal_safe_text(version));
        }
        for conflict in &host.conflicts {
            let _ = writeln!(output, "  Conflict: {}", terminal_safe_text(conflict));
        }
        for action in &host.user_actions {
            let _ = writeln!(output, "  Next: {}", terminal_safe_text(action));
        }
    }

    if plan.mode == SetupMode::DryRun && plan.ready {
        let _ = writeln!(
            output,
            "\nPlan SHA-256: {}\nNo files changed. Re-run this exact command with `--apply --plan-sha256 {}` after reviewing the plan.",
            terminal_safe_text(&plan.plan_sha256),
            terminal_safe_text(&plan.plan_sha256),
        );
    } else if plan.changed {
        let _ = writeln!(output, "\nChanges completed.");
    } else {
        let _ = writeln!(output, "\nNo changes were necessary.");
    }

    output
}

const fn map_target(target: AgentSetupTarget) -> SetupTarget {
    match target {
        AgentSetupTarget::Codex => SetupTarget::Codex,
        AgentSetupTarget::Claude => SetupTarget::Claude,
        AgentSetupTarget::All => SetupTarget::All,
    }
}

const fn map_scope(scope: AgentSetupScope) -> SetupScope {
    match scope {
        AgentSetupScope::User => SetupScope::User,
        AgentSetupScope::Project => SetupScope::Project,
    }
}

fn write_json(document: &Value) {
    println!("{}", heyfood_agent_contract::canonical_json(document));
}

fn failure(
    machine: bool,
    kind: &'static str,
    message: &'static str,
    hint: &'static str,
) -> ExitCode {
    let rendered = heyfood_cli::render_error(kind, message, Some(hint), machine)
        .expect("the stable error envelope is serializable");
    if machine {
        print!("{rendered}");
    } else {
        eprint!("{rendered}");
    }
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use heyfood_agent_setup::{
        BinaryIdentity, HostSetupPlan, McpRegistrationPlan, SetupMode, SetupOperation, SetupPlan,
        SetupScope, SetupTarget, SkillPackageIdentity,
    };

    use super::render_setup_plan;

    #[test]
    fn human_setup_plan_is_truthful_bounded_and_terminal_safe() {
        let plan = SetupPlan {
            schema_version: 1,
            operation: SetupOperation::Install,
            mode: SetupMode::DryRun,
            target: SetupTarget::Codex,
            scope: SetupScope::Project,
            project_root: Some(PathBuf::from("/tmp/project")),
            binary: BinaryIdentity {
                path: PathBuf::from("/tmp/heyfood"),
                sha256: "a".repeat(64),
                version: "0.6.0",
            },
            package: SkillPackageIdentity {
                name: "heyfood",
                version: "0.6.0",
                sha256: "b".repeat(64),
                files: 6,
            },
            plan_sha256: "c".repeat(64),
            ready: false,
            changed: false,
            hosts: vec![HostSetupPlan {
                host: heyfood_agent_setup::Host::Codex,
                host_executable: Some(PathBuf::from("/usr/bin/codex")),
                host_version: Some("codex-cli 0.145.0-alpha.18".to_owned()),
                compatible_version: "codex-cli 0.145.0-alpha.18",
                compatibility: "compatible",
                skill_path: PathBuf::from("/tmp/project/.agents/skills/heyfood"),
                receipt_path: PathBuf::from("/tmp/receipt"),
                mcp: McpRegistrationPlan {
                    name: "heyfood",
                    transport: "stdio",
                    command: PathBuf::from("/tmp/heyfood"),
                    arguments: vec!["mcp".to_owned(), "serve".to_owned()],
                    environment: std::collections::BTreeMap::new(),
                    environment_policy_sha256: "d".repeat(64),
                    configuration_scope: "unsupported",
                    action: "conflict",
                },
                action: "conflict",
                conflicts: vec!["user-owned\u{1b}[31m files remain".to_owned()],
                user_actions: vec!["resolve the conflict".to_owned()],
            }],
        };

        let rendered = render_setup_plan(&plan);
        assert!(rendered.contains("Agent integration setup (dry run): blocked"));
        assert!(rendered.contains("Codex: conflict · compatible"));
        assert!(rendered.contains("user-owned[31m files remain"));
        assert!(rendered.contains("resolve the conflict"));
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains("Re-run this exact command"));
        assert!(rendered.len() < 2_048);
    }
}
