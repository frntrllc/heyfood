//! Repository policy validators.

#![forbid(unsafe_code)]

mod json;
mod sha256;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use cargo_metadata::{Metadata, MetadataCommand};
use json::Json;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LedgerReport {
    pub entries: usize,
    pub mapped: usize,
    pub unmapped: usize,
    pub pytest_nodes: usize,
    pub non_pytest_invariants: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContractReport {
    pub endpoints: usize,
    pub browser_navigations: usize,
    pub local_listeners: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssetReport {
    pub assets: usize,
    pub pending_reviews: usize,
}

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

/// Validate the immutable Python migration freeze.
///
/// Phase 0 intentionally permits `unmapped` entries. The report makes that
/// debt visible; later DG-R5 policy can require `unmapped == 0` without
/// weakening the freeze validator.
pub fn verify_migration_ledger(root: &Path) -> Result<LedgerReport, String> {
    verify_frozen_schema(
        root,
        "schemas/migration-ledger.v1.schema.json",
        "https://hello.food/schemas/migration-ledger.v1.schema.json",
        "5bdd88124d097df2f8c6ec474ad4caeb0f0fcb0dc07c0da24d73a6a98cbbd574",
    )?;
    let ledger = read_json(root, "tests/migration/python-test-ledger.json")?;
    let ledger = ledger.object("migration ledger")?;
    exact_keys(
        ledger,
        &[
            "$schema",
            "schema_version",
            "baseline",
            "summary",
            "entries",
        ],
        "migration ledger",
    )?;
    expect_string(
        ledger,
        "$schema",
        "../../schemas/migration-ledger.v1.schema.json",
        "migration ledger",
    )?;
    expect_usize(ledger, "schema_version", 1, "migration ledger")?;

    let baseline = field(ledger, "baseline", "migration ledger")?.object("ledger baseline")?;
    exact_keys(
        baseline,
        &[
            "commit_sha",
            "tree_sha",
            "python_version",
            "pytest_node_ids_path",
            "pytest_node_count",
            "pytest_node_ids_sha256",
            "non_pytest_inventory_path",
            "non_pytest_invariant_count",
        ],
        "ledger baseline",
    )?;
    let baseline_sha = required_string(baseline, "commit_sha", "ledger baseline")?;
    validate_git_sha(baseline_sha, "baseline.commit_sha")?;
    validate_git_sha(
        required_string(baseline, "tree_sha", "ledger baseline")?,
        "baseline.tree_sha",
    )?;
    nonempty(
        required_string(baseline, "python_version", "ledger baseline")?,
        "baseline.python_version",
    )?;

    let node_path = safe_relative_path(required_string(
        baseline,
        "pytest_node_ids_path",
        "ledger baseline",
    )?)?;
    let node_bytes = read_bytes(root, &node_path)?;
    let declared_node_hash =
        required_string(baseline, "pytest_node_ids_sha256", "ledger baseline")?;
    validate_sha256(declared_node_hash, "baseline.pytest_node_ids_sha256")?;
    expect_hash(&node_bytes, declared_node_hash, &node_path)?;
    let node_text = std::str::from_utf8(&node_bytes)
        .map_err(|error| format!("{} is not UTF-8: {error}", node_path.display()))?;
    let nodes: Vec<_> = node_text.lines().collect();
    let node_count = field(baseline, "pytest_node_count", "ledger baseline")?
        .usize("baseline.pytest_node_count")?;
    if nodes.len() != node_count || nodes.iter().any(|node| node.is_empty()) {
        return Err(format!(
            "pytest node inventory count/content differs: declared {node_count}, actual {}",
            nodes.len()
        ));
    }
    unique_strings(nodes.iter().copied(), "pytest node IDs")?;

    let inventory_path = safe_relative_path(required_string(
        baseline,
        "non_pytest_inventory_path",
        "ledger baseline",
    )?)?;
    let inventory = read_json_path(root, &inventory_path)?;
    let inventory = inventory.object("non-pytest inventory")?;
    exact_keys(
        inventory,
        &[
            "$schema",
            "schema_version",
            "baseline_sha",
            "inventory_count",
            "invariants",
        ],
        "non-pytest inventory",
    )?;
    expect_string(
        inventory,
        "$schema",
        "../../schemas/migration-ledger.v1.schema.json#/$defs/invariantInventoryDocument",
        "non-pytest inventory",
    )?;
    expect_usize(inventory, "schema_version", 1, "non-pytest inventory")?;
    expect_string(
        inventory,
        "baseline_sha",
        baseline_sha,
        "non-pytest inventory",
    )?;
    let invariants =
        field(inventory, "invariants", "non-pytest inventory")?.array("non-pytest invariants")?;
    let inventory_count =
        field(inventory, "inventory_count", "non-pytest inventory")?.usize("inventory_count")?;
    let baseline_inventory_count =
        field(baseline, "non_pytest_invariant_count", "ledger baseline")?
            .usize("baseline.non_pytest_invariant_count")?;
    if invariants.len() != inventory_count || inventory_count != baseline_inventory_count {
        return Err(format!(
            "non-pytest inventory counts differ: entries {}, document {inventory_count}, baseline {baseline_inventory_count}",
            invariants.len()
        ));
    }

    let mut expected_ids = BTreeSet::new();
    for node in &nodes {
        expected_ids.insert(format!("pytest::{node}"));
    }
    for (index, invariant) in invariants.iter().enumerate() {
        let context = format!("invariants[{index}]");
        let invariant = invariant.object(&context)?;
        exact_keys(
            invariant,
            &[
                "invariant_id",
                "category",
                "source_path",
                "source_locator",
                "source_sha256",
                "contract",
            ],
            &context,
        )?;
        let id = required_nonempty_string(invariant, "invariant_id", &context)?;
        let category = required_string(invariant, "category", &context)?;
        validate_category(category, false, &format!("{context}.category"))?;
        required_nonempty_string(invariant, "source_path", &context)?;
        required_nonempty_string(invariant, "source_locator", &context)?;
        validate_sha256(
            required_string(invariant, "source_sha256", &context)?,
            &format!("{context}.source_sha256"),
        )?;
        required_nonempty_string(invariant, "contract", &context)?;
        if !expected_ids.insert(id.to_owned()) {
            return Err(format!("duplicate invariant ID {id:?}"));
        }
    }

    let entries = field(ledger, "entries", "migration ledger")?.array("ledger entries")?;
    let mut actual_ids = BTreeSet::new();
    let mut mapped = 0;
    let mut unmapped = 0;
    for (index, entry) in entries.iter().enumerate() {
        let context = format!("entries[{index}]");
        let entry = entry.object(&context)?;
        exact_keys(
            entry,
            &[
                "baseline_sha",
                "invariant_id",
                "original",
                "category",
                "migration_status",
                "disposition",
                "replacements",
                "rationale",
                "owner",
                "reviewer",
                "reviewed_commit_sha",
            ],
            &context,
        )?;
        expect_string(entry, "baseline_sha", baseline_sha, &context)?;
        let id = required_nonempty_string(entry, "invariant_id", &context)?;
        required_nonempty_string(entry, "original", &context)?;
        validate_category(
            required_string(entry, "category", &context)?,
            true,
            &format!("{context}.category"),
        )?;
        if !actual_ids.insert(id.to_owned()) {
            return Err(format!("duplicate ledger invariant ID {id:?}"));
        }
        let replacements = field(entry, "replacements", &context)?.array("replacements")?;
        unique_strings(
            replacements
                .iter()
                .map(|value| value.string("replacement"))
                .collect::<Result<Vec<_>, _>>()?,
            &format!("{context}.replacements"),
        )?;
        match required_string(entry, "migration_status", &context)? {
            "unmapped" => {
                unmapped += 1;
                if !matches!(field(entry, "disposition", &context)?, Json::Null)
                    || !replacements.is_empty()
                    || !null_fields(
                        entry,
                        &["rationale", "owner", "reviewer", "reviewed_commit_sha"],
                    )?
                {
                    return Err(format!("{context} has metadata on an unmapped entry"));
                }
            }
            "mapped" => {
                mapped += 1;
                let disposition = required_string(entry, "disposition", &context)?;
                if !matches!(
                    disposition,
                    "rust_test"
                        | "fixture"
                        | "installed_artifact_qualification"
                        | "platform_qualification"
                        | "retired"
                ) || replacements.is_empty()
                {
                    return Err(format!("{context} has an invalid mapped disposition"));
                }
                for name in ["rationale", "owner", "reviewer"] {
                    required_nonempty_string(entry, name, &context)?;
                }
                validate_git_sha(
                    required_string(entry, "reviewed_commit_sha", &context)?,
                    &format!("{context}.reviewed_commit_sha"),
                )?;
                if disposition == "fixture" {
                    for replacement in replacements {
                        let path = safe_relative_path(replacement.string("fixture replacement")?)?;
                        if !root.join(&path).is_file() {
                            return Err(format!(
                                "mapped fixture does not exist: {}",
                                path.display()
                            ));
                        }
                    }
                }
            }
            status => return Err(format!("{context} has invalid migration_status {status:?}")),
        }
    }
    if actual_ids != expected_ids {
        return Err(set_difference(
            "ledger invariant IDs",
            &expected_ids,
            &actual_ids,
        ));
    }

    let summary = field(ledger, "summary", "migration ledger")?.object("ledger summary")?;
    exact_keys(
        summary,
        &["entry_count", "mapped_count", "unmapped_count"],
        "ledger summary",
    )?;
    expect_usize(summary, "entry_count", entries.len(), "ledger summary")?;
    expect_usize(summary, "mapped_count", mapped, "ledger summary")?;
    expect_usize(summary, "unmapped_count", unmapped, "ledger summary")?;

    Ok(LedgerReport {
        entries: entries.len(),
        mapped,
        unmapped,
        pytest_nodes: node_count,
        non_pytest_invariants: inventory_count,
    })
}

/// Validate the stable outbound-surface contract and its frozen compatibility provenance.
pub fn verify_stable_contracts(root: &Path) -> Result<ContractReport, String> {
    let contract = read_json(root, "fixtures/contracts/called-endpoints.json")?;
    let contract = contract.object("called-endpoints contract")?;
    exact_keys(
        contract,
        &[
            "$comment",
            "schema_version",
            "provenance",
            "endpoints",
            "browser_navigations",
            "local_listeners",
        ],
        "called-endpoints contract",
    )?;
    expect_usize(contract, "schema_version", 1, "called-endpoints contract")?;
    let provenance = field(contract, "provenance", "called-endpoints contract")?
        .object("contract provenance")?;
    exact_keys(
        provenance,
        &[
            "baseline_sha",
            "baseline_tree",
            "compatibility_fixture",
            "compatibility_fixture_sha256",
            "compatibility_endpoint_count",
            "capture_tool",
            "capture_tool_version",
        ],
        "contract provenance",
    )?;
    validate_git_sha(
        required_string(provenance, "baseline_sha", "contract provenance")?,
        "provenance.baseline_sha",
    )?;
    validate_git_sha(
        required_string(provenance, "baseline_tree", "contract provenance")?,
        "provenance.baseline_tree",
    )?;
    required_nonempty_string(provenance, "capture_tool", "contract provenance")?;
    expect_usize(provenance, "capture_tool_version", 1, "contract provenance")?;

    let compatibility_path = safe_relative_path(required_string(
        provenance,
        "compatibility_fixture",
        "contract provenance",
    )?)?;
    let compatibility_bytes = read_bytes(root, &compatibility_path)?;
    let compatibility_hash = required_string(
        provenance,
        "compatibility_fixture_sha256",
        "contract provenance",
    )?;
    validate_sha256(
        compatibility_hash,
        "provenance.compatibility_fixture_sha256",
    )?;
    expect_hash(
        &compatibility_bytes,
        compatibility_hash,
        &compatibility_path,
    )?;
    let compatibility = json::parse(
        std::str::from_utf8(&compatibility_bytes)
            .map_err(|error| format!("{} is not UTF-8: {error}", compatibility_path.display()))?,
    )?;
    let compatibility = compatibility.object("compatibility fixture")?;
    exact_keys(
        compatibility,
        &["$comment", "schema_version", "endpoints"],
        "compatibility fixture",
    )?;
    expect_usize(compatibility, "schema_version", 1, "compatibility fixture")?;
    let compatibility_endpoints = field(compatibility, "endpoints", "compatibility fixture")?
        .array("compatibility endpoints")?;
    let declared_compatibility_count = field(
        provenance,
        "compatibility_endpoint_count",
        "contract provenance",
    )?
    .usize("provenance.compatibility_endpoint_count")?;
    if compatibility_endpoints.len() != declared_compatibility_count {
        return Err(format!(
            "compatibility endpoint count differs: declared {declared_compatibility_count}, actual {}",
            compatibility_endpoints.len()
        ));
    }

    let endpoints =
        field(contract, "endpoints", "called-endpoints contract")?.array("endpoints")?;
    validate_endpoint_rows(endpoints, "endpoints")?;
    validate_endpoint_rows(compatibility_endpoints, "compatibility endpoints")?;
    let stable_keys = endpoint_keys(endpoints)?;
    let compatibility_keys = endpoint_keys(compatibility_endpoints)?;
    if !compatibility_keys.is_subset(&stable_keys) {
        return Err(set_difference(
            "stable endpoint coverage",
            &compatibility_keys,
            &stable_keys,
        ));
    }
    for compatibility_row in compatibility_endpoints {
        let compatibility_row = compatibility_row.object("compatibility endpoint")?;
        let method = required_string(compatibility_row, "method", "compatibility endpoint")?;
        let endpoint = required_string(compatibility_row, "endpoint", "compatibility endpoint")?;
        let stable_row = endpoints.iter().find(|row| {
            row.object("stable endpoint").is_ok_and(|row| {
                required_string(row, "method", "stable endpoint") == Ok(method)
                    && required_string(row, "endpoint", "stable endpoint") == Ok(endpoint)
            })
        });
        if stable_row.and_then(|row| row.object("stable endpoint").ok()) != Some(compatibility_row)
        {
            return Err(format!(
                "stable contract changed frozen compatibility row {method} {endpoint}"
            ));
        }
    }

    let browser = field(contract, "browser_navigations", "called-endpoints contract")?
        .array("browser_navigations")?;
    validate_object_rows(
        browser,
        &["source", "scheme_policy", "origin", "note"],
        "browser_navigations",
    )?;
    let listeners = field(contract, "local_listeners", "called-endpoints contract")?
        .array("local_listeners")?;
    for (index, listener) in listeners.iter().enumerate() {
        let context = format!("local_listeners[{index}]");
        let listener = listener.object(&context)?;
        exact_keys(
            listener,
            &["name", "bind", "port_policy", "routes", "note"],
            &context,
        )?;
        for name in ["name", "bind", "port_policy", "note"] {
            required_nonempty_string(listener, name, &context)?;
        }
        let routes = field(listener, "routes", &context)?.array("listener routes")?;
        validate_object_rows(routes, &["method", "path"], &format!("{context}.routes"))?;
    }

    Ok(ContractReport {
        endpoints: endpoints.len(),
        browser_navigations: browser.len(),
        local_listeners: listeners.len(),
    })
}

/// Validate Phase 0 runtime assets, schema markers, hashes, and provenance.
pub fn verify_assets(root: &Path) -> Result<AssetReport, String> {
    for (schema, id, hash) in [
        (
            "schemas/dietary-options.v2.schema.json",
            "https://hello.food/schemas/dietary-options.v2.schema.json",
            "0c97edd5e47d35ae60e7e412f9b059795a5e9aeabafe9bfc29a7e0670a85e57d",
        ),
        (
            "schemas/banner-palette.v1.schema.json",
            "https://hello.food/schemas/banner-palette.v1.schema.json",
            "bf65a49ca2672b4a1f147bc02e89b9471e5d09aa778886d7024e469c515eb77c",
        ),
        (
            "schemas/banner-frames.v1.schema.json",
            "https://hello.food/schemas/banner-frames.v1.schema.json",
            "43dedeb1c10f9c1a12319d50167d81feb3d53a42611ade0e672a2529e2c1c3b7",
        ),
    ] {
        verify_frozen_schema(root, schema, id, hash)?;
    }

    let dietary = read_json(root, "assets/dietary/dietary_options.v2.json")?;
    let dietary = dietary.object("dietary asset")?;
    exact_keys(
        dietary,
        &["version", "sections", "household_diet_extras"],
        "dietary asset",
    )?;
    expect_usize(dietary, "version", 2, "dietary asset")?;
    let sections = field(dietary, "sections", "dietary asset")?.object("dietary sections")?;
    exact_keys(
        sections,
        &[
            "health_conditions",
            "diet_style",
            "allergies",
            "ingredients_to_avoid",
            "activity_level",
            "cuisines",
        ],
        "dietary sections",
    )?;
    for name in ["health_conditions", "diet_style", "allergies", "cuisines"] {
        validate_tiered_section(field(sections, name, "dietary sections")?, name)?;
    }
    validate_custom_section(field(sections, "ingredients_to_avoid", "dietary sections")?)?;
    validate_options_section(field(sections, "activity_level", "dietary sections")?)?;
    let extras =
        field(dietary, "household_diet_extras", "dietary asset")?.array("household_diet_extras")?;
    validate_options(extras, "household_diet_extras", false)?;

    let palette = read_json(root, "assets/brand/banner.palette.json")?;
    validate_palette(&palette)?;
    let frames = read_json(root, "assets/brand/banner.frames.json")?;
    validate_frames(root, &frames)?;

    let dietary_review = validate_dietary_provenance(root)?;
    let brand_review = validate_brand_provenance(root)?;
    Ok(AssetReport {
        assets: 4,
        pending_reviews: usize::from(dietary_review) + usize::from(brand_review),
    })
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

fn validate_dietary_provenance(root: &Path) -> Result<bool, String> {
    let document = read_json(root, "assets/dietary/provenance.json")?;
    let provenance = document.object("dietary provenance")?;
    exact_keys(
        provenance,
        &[
            "schema_version",
            "asset_contract_version",
            "source_repository",
            "source_commit",
            "source_commit_clean",
            "source_path",
            "source_sha256",
            "target_path",
            "target_sha256",
            "export_tool",
            "export_tool_version",
            "review",
        ],
        "dietary provenance",
    )?;
    expect_usize(provenance, "schema_version", 1, "dietary provenance")?;
    expect_usize(
        provenance,
        "asset_contract_version",
        2,
        "dietary provenance",
    )?;
    validate_git_sha(
        required_string(provenance, "source_commit", "dietary provenance")?,
        "dietary provenance.source_commit",
    )?;
    if !field(provenance, "source_commit_clean", "dietary provenance")?
        .boolean("source_commit_clean")?
    {
        return Err("dietary provenance source_commit_clean must be true".to_owned());
    }
    for name in ["source_repository", "source_path", "export_tool"] {
        required_nonempty_string(provenance, name, "dietary provenance")?;
    }
    expect_usize(provenance, "export_tool_version", 1, "dietary provenance")?;
    validate_sha256(
        required_string(provenance, "source_sha256", "dietary provenance")?,
        "dietary provenance.source_sha256",
    )?;
    let target = safe_relative_path(required_string(
        provenance,
        "target_path",
        "dietary provenance",
    )?)?;
    let declared = required_string(provenance, "target_sha256", "dietary provenance")?;
    validate_sha256(declared, "dietary provenance.target_sha256")?;
    expect_hash(&read_bytes(root, &target)?, declared, &target)?;
    if required_string(provenance, "source_sha256", "dietary provenance")? != declared {
        return Err(
            "dietary source and target hashes must match for the exact snapshot export".to_owned(),
        );
    }
    validate_review(
        field(provenance, "review", "dietary provenance")?,
        "dietary provenance.review",
    )
}

fn validate_brand_provenance(root: &Path) -> Result<bool, String> {
    let document = read_json(root, "assets/brand/provenance.json")?;
    let provenance = document.object("brand provenance")?;
    exact_keys(
        provenance,
        &[
            "schema_version",
            "source_repository",
            "source_commit",
            "source_commit_clean",
            "sources",
            "targets",
            "export_tool",
            "export_tool_version",
            "review",
        ],
        "brand provenance",
    )?;
    expect_usize(provenance, "schema_version", 1, "brand provenance")?;
    validate_git_sha(
        required_string(provenance, "source_commit", "brand provenance")?,
        "brand provenance.source_commit",
    )?;
    if !field(provenance, "source_commit_clean", "brand provenance")?
        .boolean("source_commit_clean")?
    {
        return Err("brand provenance source_commit_clean must be true".to_owned());
    }
    for name in ["source_repository", "export_tool"] {
        required_nonempty_string(provenance, name, "brand provenance")?;
    }
    expect_usize(provenance, "export_tool_version", 1, "brand provenance")?;
    for list in ["sources", "targets"] {
        let rows = field(provenance, list, "brand provenance")?.array(list)?;
        if rows.is_empty() {
            return Err(format!("brand provenance {list} must not be empty"));
        }
        for (index, row) in rows.iter().enumerate() {
            let context = format!("brand provenance.{list}[{index}]");
            let row = row.object(&context)?;
            exact_keys(row, &["path", "sha256"], &context)?;
            let path = safe_relative_path(required_string(row, "path", &context)?)?;
            let declared = required_string(row, "sha256", &context)?;
            validate_sha256(declared, &format!("{context}.sha256"))?;
            expect_hash(&read_bytes(root, &path)?, declared, &path)?;
        }
    }
    validate_review(
        field(provenance, "review", "brand provenance")?,
        "brand provenance.review",
    )
}

fn validate_review(value: &Json, context: &str) -> Result<bool, String> {
    let review = value.object(context)?;
    exact_keys(
        review,
        &["status", "reviewer", "reviewed_commit_sha"],
        context,
    )?;
    match required_string(review, "status", context)? {
        "pending" => {
            if !null_fields(review, &["reviewer", "reviewed_commit_sha"])? {
                return Err(format!(
                    "{context} pending review must not name approval metadata"
                ));
            }
            Ok(true)
        }
        "approved" => {
            required_nonempty_string(review, "reviewer", context)?;
            validate_git_sha(
                required_string(review, "reviewed_commit_sha", context)?,
                &format!("{context}.reviewed_commit_sha"),
            )?;
            Ok(false)
        }
        status => Err(format!("{context}.status has invalid value {status:?}")),
    }
}

fn validate_palette(value: &Json) -> Result<(), String> {
    let palette = value.object("banner palette")?;
    exact_keys(
        palette,
        &["schema_version", "foreground", "accent", "accent_spans"],
        "banner palette",
    )?;
    expect_usize(palette, "schema_version", 1, "banner palette")?;
    for name in ["foreground", "accent"] {
        let color = required_string(palette, name, "banner palette")?;
        if color.len() != 7
            || !color.starts_with('#')
            || !color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!("banner palette.{name} is not a six-digit color"));
        }
    }
    validate_spans(
        field(palette, "accent_spans", "banner palette")?,
        "banner palette.accent_spans",
    )
}

fn validate_frames(root: &Path, value: &Json) -> Result<(), String> {
    let frames = value.object("banner frames")?;
    exact_keys(
        frames,
        &["$schema", "schema_version", "source", "geometry", "frames"],
        "banner frames",
    )?;
    expect_string(
        frames,
        "$schema",
        "../../schemas/banner-frames.v1.schema.json",
        "banner frames",
    )?;
    expect_usize(frames, "schema_version", 1, "banner frames")?;
    let source = field(frames, "source", "banner frames")?.object("banner frame source")?;
    exact_keys(
        source,
        &["banner_sha256", "palette_sha256"],
        "banner frame source",
    )?;
    for (name, path) in [
        ("banner_sha256", Path::new("assets/brand/banner.txt")),
        (
            "palette_sha256",
            Path::new("assets/brand/banner.palette.json"),
        ),
    ] {
        let declared = required_string(source, name, "banner frame source")?;
        validate_sha256(declared, name)?;
        expect_hash(&read_bytes(root, path)?, declared, path)?;
    }
    let geometry = field(frames, "geometry", "banner frames")?.object("banner geometry")?;
    exact_keys(
        geometry,
        &["width", "height", "encoding"],
        "banner geometry",
    )?;
    let width = positive_usize(geometry, "width", "banner geometry")?;
    let height = positive_usize(geometry, "height", "banner geometry")?;
    expect_string(geometry, "encoding", "utf-8", "banner geometry")?;
    let banner = fs::read_to_string(root.join("assets/brand/banner.txt"))
        .map_err(|error| format!("could not read banner text: {error}"))?;
    let lines: Vec<_> = banner.lines().collect();
    if lines.len() != height || lines.iter().map(|line| line.chars().count()).max() != Some(width) {
        return Err("banner geometry does not match banner.txt".to_owned());
    }
    let rows = field(frames, "frames", "banner frames")?.array("frames")?;
    if rows.is_empty() {
        return Err("banner frames must not be empty".to_owned());
    }
    for (index, row) in rows.iter().enumerate() {
        let context = format!("frames[{index}]");
        let row = row.object(&context)?;
        exact_keys(
            row,
            &[
                "index",
                "duration_ms",
                "visible_lines",
                "accent_spans",
                "plain_text_sha256",
            ],
            &context,
        )?;
        expect_usize(row, "index", index, &context)?;
        field(row, "duration_ms", &context)?.usize("duration_ms")?;
        let visible = field(row, "visible_lines", &context)?.array("visible_lines")?;
        unique_usizes(visible, height, &format!("{context}.visible_lines"))?;
        validate_spans(
            field(row, "accent_spans", &context)?,
            &format!("{context}.accent_spans"),
        )?;
        let declared = required_string(row, "plain_text_sha256", &context)?;
        validate_sha256(declared, &format!("{context}.plain_text_sha256"))?;
        if visible.len() == height {
            expect_hash(
                banner.as_bytes(),
                declared,
                Path::new("assets/brand/banner.txt"),
            )?;
        }
    }
    Ok(())
}

fn validate_tiered_section(value: &Json, context: &str) -> Result<(), String> {
    let section = value.object(context)?;
    required_keys(
        section,
        &[
            "label",
            "multi_select",
            "tier1",
            "tier2",
            "custom_placeholder",
            "custom_max",
            "custom_char_limit",
        ],
        context,
    )?;
    allowed_keys(
        section,
        &[
            "label",
            "multi_select",
            "note",
            "tier1",
            "tier2",
            "custom_placeholder",
            "custom_max",
            "custom_char_limit",
        ],
        context,
    )?;
    required_nonempty_string(section, "label", context)?;
    if !field(section, "multi_select", context)?.boolean("multi_select")? {
        return Err(format!("{context}.multi_select must be true"));
    }
    let tier1 = field(section, "tier1", context)?.array("tier1")?;
    if tier1.is_empty() {
        return Err(format!("{context}.tier1 must not be empty"));
    }
    validate_options(tier1, &format!("{context}.tier1"), true)?;
    validate_options(
        field(section, "tier2", context)?.array("tier2")?,
        &format!("{context}.tier2"),
        true,
    )?;
    required_nonempty_string(section, "custom_placeholder", context)?;
    positive_usize(section, "custom_max", context)?;
    positive_usize(section, "custom_char_limit", context)?;
    Ok(())
}

fn validate_custom_section(value: &Json) -> Result<(), String> {
    let section = value.object("ingredients_to_avoid")?;
    exact_keys(
        section,
        &[
            "label",
            "multi_select",
            "type",
            "placeholder",
            "max_items",
            "char_limit",
        ],
        "ingredients_to_avoid",
    )?;
    required_nonempty_string(section, "label", "ingredients_to_avoid")?;
    required_nonempty_string(section, "placeholder", "ingredients_to_avoid")?;
    expect_string(section, "type", "custom_only", "ingredients_to_avoid")?;
    if !field(section, "multi_select", "ingredients_to_avoid")?.boolean("multi_select")? {
        return Err("ingredients_to_avoid.multi_select must be true".to_owned());
    }
    positive_usize(section, "max_items", "ingredients_to_avoid")?;
    positive_usize(section, "char_limit", "ingredients_to_avoid")?;
    Ok(())
}

fn validate_options_section(value: &Json) -> Result<(), String> {
    let section = value.object("activity_level")?;
    exact_keys(
        section,
        &["label", "multi_select", "options"],
        "activity_level",
    )?;
    required_nonempty_string(section, "label", "activity_level")?;
    if field(section, "multi_select", "activity_level")?.boolean("multi_select")? {
        return Err("activity_level.multi_select must be false".to_owned());
    }
    let options = field(section, "options", "activity_level")?.array("activity_level.options")?;
    if options.is_empty() {
        return Err("activity_level.options must not be empty".to_owned());
    }
    validate_options(options, "activity_level.options", false)
}

fn validate_options(options: &[Json], context: &str, catalog: bool) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    for (index, option) in options.iter().enumerate() {
        let context = format!("{context}[{index}]");
        let option = option.object(&context)?;
        if catalog {
            allowed_keys(
                option,
                &[
                    "label",
                    "id",
                    "constraints",
                    "enum_key",
                    "deprecated",
                    "replaced_by",
                ],
                &context,
            )?;
        } else {
            exact_keys(option, &["label", "id"], &context)?;
        }
        required_nonempty_string(option, "label", &context)?;
        let id = required_nonempty_string(option, "id", &context)?;
        if !valid_identifier(id) || !ids.insert(id) {
            return Err(format!("{context}.id is invalid or duplicated: {id:?}"));
        }
        if let Some(constraints) = option.get("constraints") {
            let values = constraints.array(&format!("{context}.constraints"))?;
            let values = values
                .iter()
                .map(|value| value.string(&format!("{context}.constraints")))
                .collect::<Result<Vec<_>, _>>()?;
            unique_strings(values.iter().copied(), &format!("{context}.constraints"))?;
            if values.iter().any(|value| {
                !matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
                    || !value.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    })
            }) {
                return Err(format!("{context}.constraints has an invalid value"));
            }
        }
        if let Some(enum_key) = option.get("enum_key") {
            match enum_key {
                Json::Null => {}
                Json::String(value) if !value.is_empty() => {}
                _ => {
                    return Err(format!(
                        "{context}.enum_key must be null or a non-empty string"
                    ));
                }
            }
        }
        if let Some(deprecated) = option.get("deprecated") {
            deprecated.boolean(&format!("{context}.deprecated"))?;
        }
        if matches!(option.get("deprecated"), Some(Json::Bool(true))) {
            let replacements = field(option, "replaced_by", &context)?.array("replaced_by")?;
            if replacements.is_empty() {
                return Err(format!("{context}.replaced_by must not be empty"));
            }
        }
        if let Some(replacements) = option.get("replaced_by") {
            let replacements = replacements.array(&format!("{context}.replaced_by"))?;
            let replacements = replacements
                .iter()
                .map(|value| value.string(&format!("{context}.replaced_by")))
                .collect::<Result<Vec<_>, _>>()?;
            unique_strings(
                replacements.iter().copied(),
                &format!("{context}.replaced_by"),
            )?;
            if replacements.iter().any(|value| !valid_identifier(value)) {
                return Err(format!("{context}.replaced_by has an invalid identifier"));
            }
        }
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    if value.starts_with("__") && value.ends_with("__") && value.len() > 4 {
        return value[2..value.len() - 2]
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    }
    matches!(bytes.first(), Some(b'a'..=b'z'))
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

fn validate_spans(value: &Json, context: &str) -> Result<(), String> {
    let spans = value.array(context)?;
    let mut unique = BTreeSet::new();
    for (index, span) in spans.iter().enumerate() {
        let item = format!("{context}[{index}]");
        let span = span.object(&item)?;
        exact_keys(span, &["line", "start", "length"], &item)?;
        let tuple = (
            field(span, "line", &item)?.usize("line")?,
            field(span, "start", &item)?.usize("start")?,
            positive_usize(span, "length", &item)?,
        );
        if !unique.insert(tuple) {
            return Err(format!("{context} contains a duplicate span"));
        }
    }
    Ok(())
}

fn validate_endpoint_rows(rows: &[Json], context: &str) -> Result<(), String> {
    validate_object_rows(rows, &["method", "endpoint", "auth", "note"], context)?;
    for (index, row) in rows.iter().enumerate() {
        let row = row.object(context)?;
        let method = required_string(row, "method", context)?;
        if !matches!(method, "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
            return Err(format!("{context}[{index}].method is invalid"));
        }
        if !required_string(row, "endpoint", context)?.starts_with('/') {
            return Err(format!("{context}[{index}].endpoint must start with /"));
        }
    }
    endpoint_keys(rows).map(|_| ())
}

fn endpoint_keys(rows: &[Json]) -> Result<BTreeSet<String>, String> {
    let mut keys = BTreeSet::new();
    for row in rows {
        let row = row.object("endpoint")?;
        let key = format!(
            "{} {}",
            required_string(row, "method", "endpoint")?,
            required_string(row, "endpoint", "endpoint")?
        );
        if !keys.insert(key.clone()) {
            return Err(format!("duplicate endpoint {key}"));
        }
    }
    Ok(keys)
}

fn validate_object_rows(rows: &[Json], keys: &[&str], context: &str) -> Result<(), String> {
    if rows.is_empty() {
        return Err(format!("{context} must not be empty"));
    }
    for (index, row) in rows.iter().enumerate() {
        let item = format!("{context}[{index}]");
        let row = row.object(&item)?;
        exact_keys(row, keys, &item)?;
        for key in keys {
            required_nonempty_string(row, key, &item)?;
        }
    }
    Ok(())
}

fn read_json(root: &Path, path: &str) -> Result<Json, String> {
    read_json_path(root, Path::new(path))
}

fn read_json_path(root: &Path, path: &Path) -> Result<Json, String> {
    let bytes = read_bytes(root, path)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("{} is not UTF-8: {error}", path.display()))?;
    json::parse(text).map_err(|error| format!("{} is invalid JSON: {error}", path.display()))
}

fn read_bytes(root: &Path, path: &Path) -> Result<Vec<u8>, String> {
    fs::read(root.join(path)).map_err(|error| format!("could not read {}: {error}", path.display()))
}

fn field<'a>(
    object: &'a BTreeMap<String, Json>,
    name: &str,
    context: &str,
) -> Result<&'a Json, String> {
    object
        .get(name)
        .ok_or_else(|| format!("{context} is missing {name:?}"))
}

fn required_string<'a>(
    object: &'a BTreeMap<String, Json>,
    name: &str,
    context: &str,
) -> Result<&'a str, String> {
    field(object, name, context)?.string(&format!("{context}.{name}"))
}

fn required_nonempty_string<'a>(
    object: &'a BTreeMap<String, Json>,
    name: &str,
    context: &str,
) -> Result<&'a str, String> {
    let value = required_string(object, name, context)?;
    nonempty(value, &format!("{context}.{name}"))?;
    Ok(value)
}

fn nonempty(value: &str, context: &str) -> Result<(), String> {
    if value.is_empty() {
        Err(format!("{context} must not be empty"))
    } else {
        Ok(())
    }
}

fn expect_string(
    object: &BTreeMap<String, Json>,
    name: &str,
    expected: &str,
    context: &str,
) -> Result<(), String> {
    let actual = required_string(object, name, context)?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{context}.{name} must be {expected:?}, got {actual:?}"
        ))
    }
}

fn expect_usize(
    object: &BTreeMap<String, Json>,
    name: &str,
    expected: usize,
    context: &str,
) -> Result<(), String> {
    let actual = field(object, name, context)?.usize(&format!("{context}.{name}"))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{context}.{name} must be {expected}, got {actual}"))
    }
}

fn positive_usize(
    object: &BTreeMap<String, Json>,
    name: &str,
    context: &str,
) -> Result<usize, String> {
    let value = field(object, name, context)?.usize(&format!("{context}.{name}"))?;
    if value == 0 {
        Err(format!("{context}.{name} must be positive"))
    } else {
        Ok(value)
    }
}

fn required_keys(
    object: &BTreeMap<String, Json>,
    required: &[&str],
    context: &str,
) -> Result<(), String> {
    for key in required {
        if !object.contains_key(*key) {
            return Err(format!("{context} is missing {key:?}"));
        }
    }
    Ok(())
}

fn exact_keys(
    object: &BTreeMap<String, Json>,
    required: &[&str],
    context: &str,
) -> Result<(), String> {
    required_keys(object, required, context)?;
    let allowed: BTreeSet<_> = required.iter().copied().collect();
    if let Some(extra) = object.keys().find(|key| !allowed.contains(key.as_str())) {
        return Err(format!("{context} has unexpected field {extra:?}"));
    }
    Ok(())
}

fn allowed_keys(
    object: &BTreeMap<String, Json>,
    allowed: &[&str],
    context: &str,
) -> Result<(), String> {
    let allowed: BTreeSet<_> = allowed.iter().copied().collect();
    if let Some(extra) = object.keys().find(|key| !allowed.contains(key.as_str())) {
        Err(format!("{context} has unexpected field {extra:?}"))
    } else {
        Ok(())
    }
}

fn verify_frozen_schema(
    root: &Path,
    path: &str,
    expected_id: &str,
    expected_hash: &str,
) -> Result<(), String> {
    let bytes = read_bytes(root, Path::new(path))?;
    expect_hash(&bytes, expected_hash, Path::new(path))?;
    let document = json::parse(
        std::str::from_utf8(&bytes).map_err(|error| format!("{path} is not UTF-8: {error}"))?,
    )
    .map_err(|error| format!("{path} is invalid JSON: {error}"))?;
    let object = document.object(path)?;
    expect_string(
        object,
        "$schema",
        "https://json-schema.org/draft/2020-12/schema",
        path,
    )?;
    expect_string(object, "$id", expected_id, path)
}

fn null_fields(object: &BTreeMap<String, Json>, names: &[&str]) -> Result<bool, String> {
    for name in names {
        if !matches!(field(object, name, "object")?, Json::Null) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_sha256(value: &str, context: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(format!("{context} must be a lowercase SHA-256"))
    }
}

fn validate_git_sha(value: &str, context: &str) -> Result<(), String> {
    if value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(format!(
            "{context} must be a lowercase 40-character Git SHA"
        ))
    }
}

fn expect_hash(bytes: &[u8], expected: &str, path: &Path) -> Result<(), String> {
    let actual = sha256::digest_hex(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "SHA-256 mismatch for {}: declared {expected}, actual {actual}",
            path.display()
        ))
    }
}

fn validate_category(value: &str, allow_pytest: bool, context: &str) -> Result<(), String> {
    if matches!(
        value,
        "ci" | "release_script" | "schema" | "documentation" | "installed_artifact"
    ) || (allow_pytest && value == "pytest")
    {
        Ok(())
    } else {
        Err(format!("{context} has invalid category {value:?}"))
    }
}

fn safe_relative_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        Err(format!(
            "repository path must be normalized and relative: {value:?}"
        ))
    } else {
        Ok(path)
    }
}

fn unique_strings<'a>(
    values: impl IntoIterator<Item = &'a str>,
    context: &str,
) -> Result<(), String> {
    let mut unique = BTreeSet::new();
    for value in values {
        nonempty(value, context)?;
        if !unique.insert(value) {
            return Err(format!("{context} contains duplicate {value:?}"));
        }
    }
    Ok(())
}

fn unique_usizes(values: &[Json], upper_bound: usize, context: &str) -> Result<(), String> {
    let mut unique = BTreeSet::new();
    for value in values {
        let value = value.usize(context)?;
        if value >= upper_bound || !unique.insert(value) {
            return Err(format!(
                "{context} contains an out-of-range or duplicate value {value}"
            ));
        }
    }
    Ok(())
}

fn set_difference(context: &str, expected: &BTreeSet<String>, actual: &BTreeSet<String>) -> String {
    let missing: Vec<_> = expected.difference(actual).take(5).collect();
    let extra: Vec<_> = actual.difference(expected).take(5).collect();
    format!("{context} differ; missing (first 5): {missing:?}; extra (first 5): {extra:?}")
}

#[cfg(test)]
mod tests {
    use super::{
        validate_dependency_dag, verify_assets, verify_migration_ledger, verify_stable_contracts,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn scratch(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("heyfood-xtask-{label}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn copy(relative: &str, destination_root: &std::path::Path) {
        let destination = destination_root.join(relative);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(root().join(relative), destination).unwrap();
    }

    #[test]
    fn checked_in_workspace_matches_approved_dependency_dag() {
        validate_dependency_dag(&root().join("Cargo.toml"))
            .expect("checked-in workspace must match the approved dependency DAG");
    }

    #[test]
    fn checked_in_phase0_freezes_validate_with_visible_unmapped_debt() {
        let ledger = verify_migration_ledger(&root()).expect("migration freeze must validate");
        assert_eq!(
            ledger.entries,
            ledger.pytest_nodes + ledger.non_pytest_invariants
        );
        assert_eq!(ledger.mapped, 0);
        assert_eq!(ledger.unmapped, ledger.entries);
        assert_eq!(verify_stable_contracts(&root()).unwrap().endpoints, 26);
        assert_eq!(verify_assets(&root()).unwrap().pending_reviews, 2);
    }

    #[test]
    fn ledger_validator_rejects_frozen_inventory_hash_corruption() {
        let scratch = scratch("ledger-corruption");
        for path in [
            "tests/migration/python-test-ledger.json",
            "tests/migration/python-node-ids.txt",
            "tests/migration/non-pytest-invariants.json",
        ] {
            copy(path, &scratch);
        }
        fs::write(
            scratch.join("tests/migration/python-node-ids.txt"),
            "corrupted\n",
        )
        .unwrap();
        assert!(verify_migration_ledger(&scratch).is_err());
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn contract_validator_rejects_provenance_hash_corruption() {
        let scratch = scratch("contract-corruption");
        for path in [
            "fixtures/contracts/called-endpoints.json",
            "tests/fixtures/called_endpoints.json",
        ] {
            copy(path, &scratch);
        }
        let fixture = scratch.join("tests/fixtures/called_endpoints.json");
        let mut bytes = fs::read(&fixture).unwrap();
        bytes.push(b'\n');
        fs::write(fixture, bytes).unwrap();
        assert!(verify_stable_contracts(&scratch).is_err());
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn asset_validator_rejects_schema_and_target_corruption() {
        let scratch = scratch("asset-corruption");
        for path in [
            "schemas/dietary-options.v2.schema.json",
            "schemas/banner-palette.v1.schema.json",
            "schemas/banner-frames.v1.schema.json",
            "assets/dietary/dietary_options.v2.json",
            "assets/dietary/provenance.json",
            "assets/brand/banner.txt",
            "assets/brand/banner.palette.json",
            "assets/brand/banner.frames.json",
            "assets/brand/provenance.json",
            "docs/references/banner.txt",
            "docs/references/banner.palette.json",
            "docs/references/banner.ts",
        ] {
            copy(path, &scratch);
        }
        let asset = scratch.join("assets/dietary/dietary_options.v2.json");
        let corrupted =
            fs::read_to_string(&asset)
                .unwrap()
                .replacen("\"version\": 2", "\"version\": 3", 1);
        fs::write(asset, corrupted).unwrap();
        assert!(verify_assets(&scratch).is_err());

        copy("assets/dietary/dietary_options.v2.json", &scratch);
        let schema = scratch.join("schemas/dietary-options.v2.schema.json");
        let corrupted = fs::read_to_string(&schema).unwrap().replacen(
            "hello.food dietary options v2",
            "corrupted schema",
            1,
        );
        fs::write(schema, corrupted).unwrap();
        assert!(verify_assets(&scratch).is_err());
        fs::remove_dir_all(scratch).unwrap();
    }
}
