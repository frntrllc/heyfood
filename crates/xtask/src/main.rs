//! Command-line entry point for repository policy validators.

#![forbid(unsafe_code)]

use std::path::Path;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next();
    if arguments.next().is_some() {
        usage();
    }

    let root = Path::new(".");
    let result = match command.as_deref() {
        Some("dependency-dag") => xtask::validate_dependency_dag(Path::new("Cargo.toml"))
            .map(|()| "dependency DAG matches the approved Phase 0 architecture".to_owned()),
        Some("verify-migration-ledger") => xtask::verify_migration_ledger(root).map(|report| {
            format!(
                "migration ledger valid: {} entries ({} pytest + {} non-pytest), {} mapped, {} unmapped (permitted by Phase 0 freeze)",
                report.entries,
                report.pytest_nodes,
                report.non_pytest_invariants,
                report.mapped,
                report.unmapped
            )
        }),
        Some("verify-contracts" | "verify-stable-contracts") =>
            xtask::verify_stable_contracts(root).map(|report| {
            format!(
                "stable contracts valid: {} endpoints, {} browser navigations, {} local listeners",
                report.endpoints, report.browser_navigations, report.local_listeners
            )
            }),
        Some("verify-assets") => xtask::verify_assets(root).map(|report| {
            format!(
                "asset schemas, hashes, and provenance valid: {} assets; {} provenance reviews pending",
                report.assets, report.pending_reviews
            )
        }),
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

fn usage() -> ! {
    eprintln!(
        "usage: cargo xtask <dependency-dag|verify-migration-ledger|verify-contracts|verify-assets>"
    );
    std::process::exit(2);
}
