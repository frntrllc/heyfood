//! Command-line entry point for repository policy validators.

#![forbid(unsafe_code)]

use std::path::Path;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next();

    let root = Path::new(".");
    let result = match command.as_deref() {
        Some("dependency-dag") => {
            no_extra_arguments(&mut arguments);
            xtask::validate_dependency_dag(Path::new("Cargo.toml"))
                .map(|()| "dependency DAG matches the approved Phase 0 architecture".to_owned())
        }
        Some("verify-migration-ledger") => {
            no_extra_arguments(&mut arguments);
            xtask::verify_migration_ledger(root).map(|report| {
                format!(
                    "migration ledger valid: {} entries ({} pytest + {} non-pytest), {} mapped, {} unmapped (permitted by Phase 0 freeze)",
                    report.entries,
                    report.pytest_nodes,
                    report.non_pytest_invariants,
                    report.mapped,
                    report.unmapped
                )
            })
        }
        Some("verify-contracts" | "verify-stable-contracts") => {
            no_extra_arguments(&mut arguments);
            xtask::verify_stable_contracts(root).map(|report| {
                format!(
                    "stable contracts valid: {} endpoints, {} browser navigations, {} local listeners",
                    report.endpoints, report.browser_navigations, report.local_listeners
                )
            })
        }
        Some("verify-grocery-contracts") => {
            no_extra_arguments(&mut arguments);
            xtask::verify_grocery_contracts(root).map(|report| {
                format!(
                    "grocery contract freeze valid: {} Platform and Phase-A contracts; import review pending: {}",
                    report.contracts, report.review_pending
                )
            })
        }
        Some("import-grocery-contracts") => {
            if arguments.next().as_deref() != Some("--source-repo") {
                usage();
            }
            let Some(source_repository) = arguments.next() else {
                usage();
            };
            no_extra_arguments(&mut arguments);
            xtask::import_grocery_contracts(root, Path::new(&source_repository)).map(|report| {
                format!(
                    "imported and verified {} Platform and Phase-A grocery contracts",
                    report.contracts
                )
            })
        }
        Some("verify-assets") => {
            no_extra_arguments(&mut arguments);
            xtask::verify_assets(root).map(|report| {
                format!(
                    "asset schemas, hashes, and provenance valid: {} assets; {} provenance reviews pending",
                    report.assets, report.pending_reviews
                )
            })
        }
        Some("verify-assets-approved") => {
            no_extra_arguments(&mut arguments);
            xtask::verify_assets_approved(root).map(|report| {
                format!(
                    "asset provenance has independent exact-SHA approval: {} assets",
                    report.assets
                )
            })
        }
        Some("verify-phase0-evidence") => {
            no_extra_arguments(&mut arguments);
            xtask::verify_phase0_evidence(root).map(|report| {
                format!(
                    "Phase 0 inventory valid: {} requirements, {} blockers, approval {}",
                    report.requirements, report.blockers, report.review_status
                )
            })
        }
        Some("verify-phase1-evidence") => {
            no_extra_arguments(&mut arguments);
            xtask::verify_phase1_evidence(root).map(|report| {
                format!(
                    "Phase 1 inventory valid: {} requirements, {} blockers, hosted {}, review {}",
                    report.requirements,
                    report.blockers,
                    report.hosted_status,
                    report.review_status
                )
            })
        }
        Some("verify-agent-phase1-evidence") => {
            no_extra_arguments(&mut arguments);
            xtask::verify_agent_phase1_evidence(root).map(|report| {
                format!(
                    "Agent-native Phase 1 inventory valid: {} requirements, {} blockers, hosted {}, artifacts {}, review {}",
                    report.requirements,
                    report.blockers,
                    report.hosted_status,
                    report.artifact_status,
                    report.review_status
                )
            })
        }
        Some("evaluate-post-release") => {
            let evidence_directory = required_option(&mut arguments, "--evidence-dir");
            let rubric = required_option(&mut arguments, "--rubric");
            let output = required_option(&mut arguments, "--output");
            no_extra_arguments(&mut arguments);
            xtask::evaluate_post_release(
                Path::new(&evidence_directory),
                Path::new(&rubric),
                Path::new(&output),
            )
            .and_then(|report| {
                let message = format!(
                    "post-release UX evaluation: score {}/{} with {} finding(s)",
                    report.score, report.threshold, report.findings
                );
                if report.passed {
                    Ok(message)
                } else {
                    Err(message)
                }
            })
        }
        _ => usage(),
    };

    match result {
        Ok(message) => println!("{message}"),
        Err(error) => {
            eprintln!("validation failed: {error}");
            std::process::exit(1);
        }
    }
}

fn no_extra_arguments(arguments: &mut impl Iterator<Item = String>) {
    if arguments.next().is_some() {
        usage();
    }
}

fn required_option(arguments: &mut impl Iterator<Item = String>, expected: &str) -> String {
    if arguments.next().as_deref() != Some(expected) {
        usage();
    }
    arguments.next().unwrap_or_else(|| usage())
}

fn usage() -> ! {
    eprintln!(
        "usage: cargo xtask <dependency-dag|verify-migration-ledger|verify-contracts|verify-grocery-contracts|import-grocery-contracts --source-repo PATH|verify-assets|verify-assets-approved|verify-phase0-evidence|verify-phase1-evidence|verify-agent-phase1-evidence|evaluate-post-release --evidence-dir PATH --rubric PATH --output PATH>"
    );
    std::process::exit(2);
}
