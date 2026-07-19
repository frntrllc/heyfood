//! Repository policy validators.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use cargo_metadata::{Metadata, MetadataCommand};

/// Validate the checked-out workspace against the approved crate dependency DAG.
pub fn validate_dependency_dag(manifest_path: &Path) -> Result<(), String> {
    let mut command = MetadataCommand::new();
    command
        .manifest_path(manifest_path)
        .no_deps()
        .other_options(["--locked".to_owned()]);
    let metadata = command
        .exec()
        .map_err(|error| format!("could not read cargo metadata: {error}"))?;

    validate_metadata(&metadata)
}

fn validate_metadata(metadata: &Metadata) -> Result<(), String> {
    let expected = expected_workspace_dependencies();
    let workspace_ids: BTreeSet<_> = metadata.workspace_members.iter().collect();
    let workspace_names: BTreeSet<_> = metadata
        .packages
        .iter()
        .filter(|package| workspace_ids.contains(&package.id))
        .map(|package| package.name.as_str())
        .collect();
    let expected_names: BTreeSet<_> = expected.keys().copied().collect();

    if workspace_names != expected_names {
        return Err(format!(
            "workspace package set differs from the approved scaffold:\n  expected: {expected_names:?}\n  actual:   {workspace_names:?}"
        ));
    }

    for package in metadata
        .packages
        .iter()
        .filter(|package| workspace_ids.contains(&package.id))
    {
        let actual: BTreeSet<_> = package
            .dependencies
            .iter()
            .map(|dependency| dependency.name.as_str())
            .filter(|name| workspace_names.contains(name))
            .collect();
        let approved = expected
            .get(package.name.as_str())
            .expect("workspace package set was checked above");

        if &actual != approved {
            return Err(format!(
                "{} has unapproved workspace dependency edges:\n  expected: {approved:?}\n  actual:   {actual:?}",
                package.name
            ));
        }
    }

    Ok(())
}

fn expected_workspace_dependencies() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    BTreeMap::from([
        ("heyfood-core", BTreeSet::new()),
        ("heyfood-application", BTreeSet::from(["heyfood-core"])),
        (
            "heyfood-agent-runtime",
            BTreeSet::from(["heyfood-application", "heyfood-core"]),
        ),
        (
            "heyfood-platform",
            BTreeSet::from(["heyfood-application", "heyfood-core"]),
        ),
        (
            "heyfood-voice",
            BTreeSet::from(["heyfood-application", "heyfood-core"]),
        ),
        (
            "heyfood-cli",
            BTreeSet::from(["heyfood-application", "heyfood-core"]),
        ),
        (
            "heyfood-tui",
            BTreeSet::from(["heyfood-application", "heyfood-core"]),
        ),
        ("heyfood-installer", BTreeSet::from(["heyfood-core"])),
        (
            "heyfood-bin",
            BTreeSet::from([
                "heyfood-agent-runtime",
                "heyfood-application",
                "heyfood-cli",
                "heyfood-core",
                "heyfood-platform",
                "heyfood-tui",
                "heyfood-voice",
            ]),
        ),
        ("xtask", BTreeSet::new()),
    ])
}

#[cfg(test)]
mod tests {
    use super::validate_dependency_dag;
    use std::path::PathBuf;

    #[test]
    fn checked_in_workspace_matches_approved_dependency_dag() {
        let workspace_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("Cargo.toml");

        validate_dependency_dag(&workspace_manifest)
            .expect("checked-in workspace must match the approved dependency DAG");
    }
}
