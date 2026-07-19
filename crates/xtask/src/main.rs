//! Command-line entry point for repository policy validators.

#![forbid(unsafe_code)]

use std::path::Path;

fn main() {
    let mut arguments = std::env::args().skip(1);

    match (arguments.next().as_deref(), arguments.next()) {
        (Some("dependency-dag"), None) => {
            if let Err(error) = xtask::validate_dependency_dag(Path::new("Cargo.toml")) {
                eprintln!("dependency DAG validation failed: {error}");
                std::process::exit(1);
            }
            println!("dependency DAG matches the approved Phase 0 architecture");
        }
        _ => {
            eprintln!("usage: cargo xtask dependency-dag");
            std::process::exit(2);
        }
    }
}
