//! Credential-free, network-free agent self-description dispatcher.

use std::process::ExitCode;

use heyfood_cli::AgentCommand;
use serde_json::{Value, json};

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
    }
    ExitCode::SUCCESS
}

fn write_json(document: &Value) {
    println!("{}", heyfood_agent_contract::canonical_json(document));
}

fn failure(
    machine: bool,
    code: &'static str,
    message: &'static str,
    action: &'static str,
) -> ExitCode {
    if machine {
        write_json(&json!({
            "schema_version": 1,
            "ok": false,
            "error": {
                "code": code,
                "message": message,
                "retryable": false,
                "outcome_uncertain": false,
                "action": action
            }
        }));
    } else {
        eprintln!("heyfood: {message}\n\n{action}");
    }
    ExitCode::FAILURE
}
