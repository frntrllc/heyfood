//! Repository policy validators.

#![forbid(unsafe_code)]

mod json;
mod sha256;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroceryContractReport {
    pub contracts: usize,
    pub review_pending: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Phase0EvidenceReport {
    pub requirements: usize,
    pub blockers: usize,
    pub review_status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Phase1EvidenceReport {
    pub requirements: usize,
    pub blockers: usize,
    pub review_status: String,
    pub hosted_status: String,
}

const FROZEN_COMPATIBILITY_SHA: &str = "73494a57468dac83b4904ce6c390e36926f5c6fe";
const FROZEN_COMPATIBILITY_TREE: &str = "4c265cd9ae0623442dd8eba1f6f4388c4ebf5adf";
const FROZEN_COMPATIBILITY_DIGEST: &str =
    "aeb4339da0cb1c73d36892c7d57d4c8b412aa2f8efcb0aec8c855fe88f835957";
const FROZEN_COMPATIBILITY_BLOB: &[u8] =
    include_bytes!("../fixtures/called_endpoints.73494a5.json");

#[derive(Clone, Copy)]
struct GroceryContractSpec {
    name: &'static str,
    platform_contract: &'static str,
    source_commit: &'static str,
    source_path: &'static str,
    sha256: &'static str,
    target_path: &'static str,
}

const GROCERY_CONTRACTS: [GroceryContractSpec; 3] = [
    GroceryContractSpec {
        name: "c3_confirmation_contract",
        platform_contract: "C3",
        source_commit: "9e0a9f220751270da56996ba7004ae25e67b06d0",
        source_path: "backend/docs/schemas/v1/confirmation-contract.json",
        sha256: "1dfb2be6befbb53068dfa16d063c2551602b04c977f5cb1e073339d316b24430",
        target_path: "fixtures/contracts/grocery-backend/c3-confirmation-contract.json",
    },
    GroceryContractSpec {
        name: "c4_application_capabilities_contract",
        platform_contract: "C4",
        source_commit: "9e1011d75be9b919452c82cc7dd849bc3f5823a2",
        source_path: "backend/docs/schemas/v1/application-capabilities-contract.json",
        sha256: "346460ef7e0eadbd292c9ccc0eee2ec0b1595893cf2ef83f0d7e97b91f3c5dac",
        target_path: "fixtures/contracts/grocery-backend/c4-application-capabilities-contract.json",
    },
    GroceryContractSpec {
        name: "c4_scopes_contract",
        platform_contract: "C4",
        source_commit: "9e1011d75be9b919452c82cc7dd849bc3f5823a2",
        source_path: "backend/docs/schemas/v1/scopes-contract.json",
        sha256: "6ad0e04f48729148731a1a432a1c8abca1187070d7bcd9e0899f3bbef961808e",
        target_path: "fixtures/contracts/grocery-backend/c4-scopes-contract.json",
    },
];

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
    expect_string(
        provenance,
        "baseline_sha",
        FROZEN_COMPATIBILITY_SHA,
        "contract provenance",
    )?;
    expect_string(
        provenance,
        "baseline_tree",
        FROZEN_COMPATIBILITY_TREE,
        "contract provenance",
    )?;
    required_nonempty_string(provenance, "capture_tool", "contract provenance")?;
    expect_usize(provenance, "capture_tool_version", 2, "contract provenance")?;

    let compatibility_path = safe_relative_path(required_string(
        provenance,
        "compatibility_fixture",
        "contract provenance",
    )?)?;
    let compatibility_hash = required_string(
        provenance,
        "compatibility_fixture_sha256",
        "contract provenance",
    )?;
    validate_sha256(
        compatibility_hash,
        "provenance.compatibility_fixture_sha256",
    )?;
    if compatibility_hash != FROZEN_COMPATIBILITY_DIGEST {
        return Err(format!(
            "provenance.compatibility_fixture_sha256 must name the frozen baseline blob {FROZEN_COMPATIBILITY_DIGEST}"
        ));
    }
    expect_hash(
        FROZEN_COMPATIBILITY_BLOB,
        compatibility_hash,
        Path::new("crates/xtask/fixtures/called_endpoints.73494a5.json"),
    )?;
    let frozen_compatibility = json::parse(
        std::str::from_utf8(FROZEN_COMPATIBILITY_BLOB)
            .map_err(|error| format!("embedded compatibility baseline is not UTF-8: {error}"))?,
    )?;
    let frozen_compatibility = frozen_compatibility.object("embedded compatibility baseline")?;
    exact_keys(
        frozen_compatibility,
        &["$comment", "schema_version", "endpoints"],
        "embedded compatibility baseline",
    )?;
    expect_usize(
        frozen_compatibility,
        "schema_version",
        1,
        "embedded compatibility baseline",
    )?;
    let frozen_endpoints = field(
        frozen_compatibility,
        "endpoints",
        "embedded compatibility baseline",
    )?
    .array("frozen compatibility endpoints")?;
    let declared_compatibility_count = field(
        provenance,
        "compatibility_endpoint_count",
        "contract provenance",
    )?
    .usize("provenance.compatibility_endpoint_count")?;
    if frozen_endpoints.len() != declared_compatibility_count {
        return Err(format!(
            "frozen compatibility endpoint count differs: declared {declared_compatibility_count}, actual {}",
            frozen_endpoints.len()
        ));
    }

    // Python CI independently proves this embedded oracle is the exact blob at
    // baseline_sha. Rust keeps the same byte-level oracle locally so shallow
    // and CRLF checkouts can still enforce exact frozen-row preservation.
    let compatibility = read_json_path(root, &compatibility_path)?;
    let compatibility = compatibility.object("live compatibility fixture")?;
    exact_keys(
        compatibility,
        &["$comment", "schema_version", "endpoints"],
        "live compatibility fixture",
    )?;
    expect_usize(
        compatibility,
        "schema_version",
        1,
        "live compatibility fixture",
    )?;
    let compatibility_endpoints = field(compatibility, "endpoints", "live compatibility fixture")?
        .array("live compatibility endpoints")?;

    let endpoints =
        field(contract, "endpoints", "called-endpoints contract")?.array("endpoints")?;
    validate_endpoint_rows(endpoints, "endpoints")?;
    validate_endpoint_rows(frozen_endpoints, "frozen compatibility endpoints")?;
    validate_endpoint_rows(compatibility_endpoints, "live compatibility endpoints")?;
    require_exact_endpoint_rows(endpoints, frozen_endpoints, "stable contract")?;
    require_exact_endpoint_rows(
        compatibility_endpoints,
        frozen_endpoints,
        "live compatibility fixture",
    )?;

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

/// Reproduce the checked-in Platform P0 C3/C4 grocery contract freeze from an
/// exact companion-repository history. Grocery Phase A is deliberately absent
/// from the import table until its reviewed merge, deployment, and canary gates
/// make a source SHA authoritative.
pub fn import_grocery_contracts(
    root: &Path,
    source_repository: &Path,
) -> Result<GroceryContractReport, String> {
    let mut imported = Vec::with_capacity(GROCERY_CONTRACTS.len());
    for contract in GROCERY_CONTRACTS {
        let object = format!("{}:{}", contract.source_commit, contract.source_path);
        let output = Command::new("git")
            .arg("-C")
            .arg(source_repository)
            .args(["show", &object])
            .output()
            .map_err(|error| {
                format!(
                    "could not read {object} from {}: {error}",
                    source_repository.display()
                )
            })?;
        if !output.status.success() {
            return Err(format!(
                "git show {object} failed in {}: {}",
                source_repository.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let actual = sha256::repository_text_digest_hex(&output.stdout)
            .map_err(|error| format!("{object} is not UTF-8 repository text: {error}"))?;
        if actual != contract.sha256 {
            return Err(format!(
                "refusing to import {object}: expected {}, found {actual}",
                contract.sha256
            ));
        }
        imported.push((contract.target_path, output.stdout));
    }

    for (target, bytes) in imported {
        let target = root.join(target);
        let parent = target
            .parent()
            .ok_or_else(|| format!("grocery contract target {} has no parent", target.display()))?;
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "could not create grocery contract directory {}: {error}",
                parent.display()
            )
        })?;
        fs::write(&target, bytes).map_err(|error| {
            format!(
                "could not write imported grocery contract {}: {error}",
                target.display()
            )
        })?;
    }

    verify_grocery_contracts(root)
}

/// Validate the deterministic Platform P0 C3/C4 grocery contract mirror and
/// prove that unfinished Grocery Phase A remains explicitly provisional.
pub fn verify_grocery_contracts(root: &Path) -> Result<GroceryContractReport, String> {
    let document = read_json(root, "fixtures/contracts/grocery-backend/provenance.json")?;
    let provenance = document.object("grocery contract provenance")?;
    exact_keys(
        provenance,
        &[
            "schema_version",
            "source_repository",
            "import_command",
            "contracts",
            "external_dependencies",
            "review",
        ],
        "grocery contract provenance",
    )?;
    expect_usize(
        provenance,
        "schema_version",
        1,
        "grocery contract provenance",
    )?;
    expect_string(
        provenance,
        "source_repository",
        "https://github.com/frntrllc/hellofood.git",
        "grocery contract provenance",
    )?;
    expect_string(
        provenance,
        "import_command",
        "cargo xtask import-grocery-contracts --source-repo PATH",
        "grocery contract provenance",
    )?;

    let contracts = field(provenance, "contracts", "grocery contract provenance")?
        .array("grocery contract provenance.contracts")?;
    if contracts.len() != GROCERY_CONTRACTS.len() {
        return Err(format!(
            "grocery contract provenance must contain exactly {} merged C3/C4 contracts",
            GROCERY_CONTRACTS.len()
        ));
    }
    for (index, (contract, expected)) in contracts.iter().zip(GROCERY_CONTRACTS.iter()).enumerate()
    {
        let context = format!("grocery contract provenance.contracts[{index}]");
        let contract = contract.object(&context)?;
        exact_keys(
            contract,
            &[
                "name",
                "platform_contract",
                "source_commit",
                "source_path",
                "source_sha256",
                "target_path",
                "target_sha256",
                "freeze_kind",
            ],
            &context,
        )?;
        for (name, value) in [
            ("name", expected.name),
            ("platform_contract", expected.platform_contract),
            ("source_commit", expected.source_commit),
            ("source_path", expected.source_path),
            ("source_sha256", expected.sha256),
            ("target_path", expected.target_path),
            ("target_sha256", expected.sha256),
            ("freeze_kind", "exact_copy"),
        ] {
            expect_string(contract, name, value, &context)?;
        }
        validate_git_sha(expected.source_commit, &format!("{context}.source_commit"))?;
        validate_sha256(expected.sha256, &format!("{context}.source_sha256"))?;
        let target = safe_relative_path(expected.target_path)?;
        expect_hash(&read_bytes(root, &target)?, expected.sha256, &target)?;
    }

    let dependencies = field(
        provenance,
        "external_dependencies",
        "grocery contract provenance",
    )?
    .object("grocery contract provenance.external_dependencies")?;
    exact_keys(
        dependencies,
        &["platform_p0", "grocery_phase_a"],
        "grocery contract provenance.external_dependencies",
    )?;
    let platform = field(
        dependencies,
        "platform_p0",
        "grocery contract provenance.external_dependencies",
    )?
    .array("grocery contract provenance.external_dependencies.platform_p0")?;
    let expected_platform = [
        (
            "C1",
            "4b7bcdfaf80053c087f5aa68fea8bd5f78732160",
            "merged_external_dependency",
        ),
        (
            "C2",
            "fa23437c324b4e5d3d1c433b9c933b9f2dc2cbca",
            "merged_external_dependency",
        ),
        (
            "C3",
            "9e0a9f220751270da56996ba7004ae25e67b06d0",
            "merged_and_frozen",
        ),
        (
            "C4",
            "9e1011d75be9b919452c82cc7dd849bc3f5823a2",
            "merged_and_frozen",
        ),
    ];
    if platform.len() != expected_platform.len() {
        return Err("grocery Platform P0 dependency ledger must contain C1-C4".to_owned());
    }
    for (index, (row, (id, commit, status))) in platform.iter().zip(expected_platform).enumerate() {
        let context =
            format!("grocery contract provenance.external_dependencies.platform_p0[{index}]");
        let row = row.object(&context)?;
        exact_keys(row, &["id", "commit", "status"], &context)?;
        expect_string(row, "id", id, &context)?;
        expect_string(row, "commit", commit, &context)?;
        validate_git_sha(commit, &format!("{context}.commit"))?;
        expect_string(row, "status", status, &context)?;
    }

    let phase_a = field(
        dependencies,
        "grocery_phase_a",
        "grocery contract provenance.external_dependencies",
    )?
    .object("grocery contract provenance.external_dependencies.grocery_phase_a")?;
    exact_keys(
        phase_a,
        &[
            "source_pr",
            "observed_head_sha",
            "observed_merge_sha",
            "status",
            "authoritative_source_sha",
            "aggregate_digest",
            "required_before_import",
        ],
        "grocery contract provenance.external_dependencies.grocery_phase_a",
    )?;
    expect_usize(
        phase_a,
        "source_pr",
        107,
        "grocery contract provenance.external_dependencies.grocery_phase_a",
    )?;
    validate_git_sha(
        required_string(
            phase_a,
            "observed_head_sha",
            "grocery contract provenance.external_dependencies.grocery_phase_a",
        )?,
        "grocery_phase_a.observed_head_sha",
    )?;
    expect_string(
        phase_a,
        "observed_head_sha",
        "8cd7baf2c683bf5ad286af32c26d96bdb1742f86",
        "grocery contract provenance.external_dependencies.grocery_phase_a",
    )?;
    expect_string(
        phase_a,
        "observed_merge_sha",
        "70d79bf6d859ff7d45738663b52a9a1074e62738",
        "grocery contract provenance.external_dependencies.grocery_phase_a",
    )?;
    validate_git_sha(
        required_string(
            phase_a,
            "observed_merge_sha",
            "grocery contract provenance.external_dependencies.grocery_phase_a",
        )?,
        "grocery_phase_a.observed_merge_sha",
    )?;
    expect_string(
        phase_a,
        "status",
        "merged_not_deployed_not_imported",
        "grocery contract provenance.external_dependencies.grocery_phase_a",
    )?;
    if !null_fields(phase_a, &["authoritative_source_sha", "aggregate_digest"])? {
        return Err(
            "provisional Grocery Phase A must not claim an authoritative SHA or digest".to_owned(),
        );
    }
    let gates = field(
        phase_a,
        "required_before_import",
        "grocery contract provenance.external_dependencies.grocery_phase_a",
    )?
    .array(
        "grocery contract provenance.external_dependencies.grocery_phase_a.required_before_import",
    )?;
    let gates: Vec<_> = gates
        .iter()
        .map(|gate| gate.string("grocery Phase A import gate"))
        .collect::<Result<_, _>>()?;
    if gates
        != [
            "production_095_to_096",
            "exact_grocery_merge_deployed_and_digest_regenerated",
            "grocery_v1_capability_and_live_canary",
        ]
    {
        return Err("grocery Phase A import gates differ from the authoritative plan".to_owned());
    }

    let review_pending = validate_review(
        field(provenance, "review", "grocery contract provenance")?,
        "grocery contract provenance.review",
    )?;
    Ok(GroceryContractReport {
        contracts: contracts.len(),
        review_pending,
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

/// Require the same byte/provenance checks as [`verify_assets`], plus an
/// independent reviewer and the exact commit they reviewed. This command is
/// intentionally separate from the freeze-integrity command: pending metadata
/// is honest during preparation, but it can never pass an approval gate.
pub fn verify_assets_approved(root: &Path) -> Result<AssetReport, String> {
    let report = verify_assets(root)?;
    if report.pending_reviews != 0 {
        return Err(format!(
            "{} asset provenance reviews remain pending; record an independent reviewer and the exact reviewed commit before approval",
            report.pending_reviews
        ));
    }
    Ok(report)
}

/// Validate the machine-readable Phase 0 inventory without turning an
/// unresolved external dependency into an invented contract.
pub fn verify_phase0_evidence(root: &Path) -> Result<Phase0EvidenceReport, String> {
    let document = read_json(
        root,
        "docs/release-evidence/rust-phase0/phase0-inventory.json",
    )?;
    let inventory = document.object("Phase 0 inventory")?;
    exact_keys(
        inventory,
        &[
            "schema_version",
            "evidence_date",
            "rust_lineage",
            "requirements",
            "external_contracts",
            "review",
        ],
        "Phase 0 inventory",
    )?;
    expect_usize(inventory, "schema_version", 1, "Phase 0 inventory")?;
    required_nonempty_string(inventory, "evidence_date", "Phase 0 inventory")?;

    let lineage =
        field(inventory, "rust_lineage", "Phase 0 inventory")?.object("Phase 0 rust_lineage")?;
    exact_keys(
        lineage,
        &[
            "repository",
            "branch",
            "code_commit_sha",
            "evidence_commit_sha",
        ],
        "Phase 0 rust_lineage",
    )?;
    for name in ["repository", "branch"] {
        required_nonempty_string(lineage, name, "Phase 0 rust_lineage")?;
    }
    validate_git_sha(
        required_string(lineage, "code_commit_sha", "Phase 0 rust_lineage")?,
        "rust_lineage.code_commit_sha",
    )?;
    if !matches!(
        field(lineage, "evidence_commit_sha", "Phase 0 rust_lineage")?,
        Json::Null
    ) {
        validate_git_sha(
            required_string(lineage, "evidence_commit_sha", "Phase 0 rust_lineage")?,
            "rust_lineage.evidence_commit_sha",
        )?;
    }

    let requirements =
        field(inventory, "requirements", "Phase 0 inventory")?.array("Phase 0 requirements")?;
    if requirements.is_empty() {
        return Err("Phase 0 requirements must not be empty".to_owned());
    }
    let mut ids = BTreeSet::new();
    let mut blockers = 0;
    for (index, requirement) in requirements.iter().enumerate() {
        let context = format!("Phase 0 requirements[{index}]");
        let requirement = requirement.object(&context)?;
        exact_keys(
            requirement,
            &["id", "status", "evidence", "blocker"],
            &context,
        )?;
        let id = required_nonempty_string(requirement, "id", &context)?;
        if !ids.insert(id) {
            return Err(format!("duplicate Phase 0 requirement ID {id:?}"));
        }
        let evidence = field(requirement, "evidence", &context)?.array("requirement evidence")?;
        if evidence.is_empty() {
            return Err(format!("{context}.evidence must not be empty"));
        }
        for value in evidence {
            required_path_exists(root, value.string("requirement evidence")?, &context)?;
        }
        match required_string(requirement, "status", &context)? {
            "satisfied" => {
                if !matches!(field(requirement, "blocker", &context)?, Json::Null) {
                    return Err(format!(
                        "{context} satisfied requirement must not have a blocker"
                    ));
                }
            }
            "blocked" => {
                blockers += 1;
                required_nonempty_string(requirement, "blocker", &context)?;
            }
            status => return Err(format!("{context}.status has invalid value {status:?}")),
        }
    }

    let contracts = field(inventory, "external_contracts", "Phase 0 inventory")?
        .object("Phase 0 external_contracts")?;
    exact_keys(
        contracts,
        &["hellofood_repository", "grocery", "kroger", "health"],
        "Phase 0 external_contracts",
    )?;
    required_nonempty_string(
        contracts,
        "hellofood_repository",
        "Phase 0 external_contracts",
    )?;
    let grocery = field(contracts, "grocery", "Phase 0 external_contracts")?
        .object("Phase 0 grocery contract")?;
    exact_keys(
        grocery,
        &[
            "observed_branch",
            "observed_head_sha",
            "observed_main_sha",
            "merged_prerequisite_shas",
            "phase_a_status",
            "authoritative_contract_sha",
            "authoritative_contract_digest",
            "blocker",
        ],
        "Phase 0 grocery contract",
    )?;
    required_nonempty_string(grocery, "observed_branch", "Phase 0 grocery contract")?;
    validate_git_sha(
        required_string(grocery, "observed_head_sha", "Phase 0 grocery contract")?,
        "grocery.observed_head_sha",
    )?;
    validate_git_sha(
        required_string(grocery, "observed_main_sha", "Phase 0 grocery contract")?,
        "grocery.observed_main_sha",
    )?;
    let prerequisites = field(
        grocery,
        "merged_prerequisite_shas",
        "Phase 0 grocery contract",
    )?
    .array("grocery prerequisite SHAs")?;
    if prerequisites.len() != 4 {
        return Err("grocery merged_prerequisite_shas must record C1-C4".to_owned());
    }
    for sha in prerequisites {
        validate_git_sha(
            sha.string("grocery prerequisite SHA")?,
            "grocery prerequisite SHA",
        )?;
    }
    match required_string(grocery, "phase_a_status", "Phase 0 grocery contract")? {
        "blocked_uncommitted"
        | "open_conflicting_checks_failed_contract_corrections_required"
        | "open_conflicting_green_contract_corrections_and_migration_096_required"
        | "superseded_by_pr_107_mergeable_096_candidate_hosted_gates_not_green"
        | "pr_107_mergeable_096_ci_green_public_preview_failed_provisional"
        | "pr_107_f5_catalog_candidate_checks_in_progress_provisional"
        | "pr_107_8cd_pg18_candidate_checks_in_progress_provisional"
        | "pr_107_merged_production_095_activation_gated" => {
            if !null_fields(
                grocery,
                &[
                    "authoritative_contract_sha",
                    "authoritative_contract_digest",
                ],
            )? {
                return Err(
                    "blocked grocery Phase A must not claim an authoritative SHA or digest"
                        .to_owned(),
                );
            }
            required_nonempty_string(grocery, "blocker", "Phase 0 grocery contract")?;
        }
        "reviewed" => {
            validate_git_sha(
                required_string(
                    grocery,
                    "authoritative_contract_sha",
                    "Phase 0 grocery contract",
                )?,
                "grocery.authoritative_contract_sha",
            )?;
            validate_sha256(
                required_string(
                    grocery,
                    "authoritative_contract_digest",
                    "Phase 0 grocery contract",
                )?,
                "grocery.authoritative_contract_digest",
            )?;
            if !matches!(
                field(grocery, "blocker", "Phase 0 grocery contract")?,
                Json::Null
            ) {
                return Err("reviewed grocery Phase A must not have a blocker".to_owned());
            }
        }
        status => {
            return Err(format!(
                "grocery.phase_a_status has invalid value {status:?}"
            ));
        }
    }

    let kroger = field(contracts, "kroger", "Phase 0 external_contracts")?
        .object("Phase 0 Kroger contract")?;
    exact_keys(
        kroger,
        &[
            "provider_foundation_pr",
            "provider_binding_pr",
            "security_d2_status",
            "status",
        ],
        "Phase 0 Kroger contract",
    )?;
    if !null_fields(kroger, &["provider_foundation_pr", "provider_binding_pr"])? {
        return Err(
            "Kroger PR fields must remain null until an observed PR is recorded".to_owned(),
        );
    }
    expect_string(
        kroger,
        "security_d2_status",
        "required_before_provider_token_storage",
        "Phase 0 Kroger contract",
    )?;
    expect_string(
        kroger,
        "status",
        "blocked_on_phase_a_and_security_d2",
        "Phase 0 Kroger contract",
    )?;

    let health = field(contracts, "health", "Phase 0 external_contracts")?
        .object("Phase 0 health contract")?;
    exact_keys(
        health,
        &[
            "h1_h2_implementation_pr",
            "h1_h2_source_sha",
            "h3_backend_implementation_pr",
            "h3_backend_source_sha",
            "h3_mobile_implementation_pr",
            "h3_mobile_source_sha",
            "provenance_path",
            "status",
        ],
        "Phase 0 health contract",
    )?;
    for (name, value) in [
        ("h1_h2_implementation_pr", 79),
        ("h3_backend_implementation_pr", 96),
        ("h3_mobile_implementation_pr", 95),
    ] {
        expect_usize(health, name, value, "Phase 0 health contract")?;
    }
    for (name, value) in [
        (
            "h1_h2_source_sha",
            "7cfadc55c103257b588b237c65fe7b5031a3f745",
        ),
        (
            "h3_backend_source_sha",
            "400c5cafb3beb0237e75f85e93d228fbbbd3dadf",
        ),
        (
            "h3_mobile_source_sha",
            "dbea9c3cc8af4610b7b6bf3f3e64ad44e7fe428a",
        ),
    ] {
        expect_string(health, name, value, "Phase 0 health contract")?;
    }
    required_path_exists(
        root,
        required_string(health, "provenance_path", "Phase 0 health contract")?,
        "Phase 0 health contract",
    )?;
    expect_string(
        health,
        "status",
        "merged_contracts_frozen_provider_neutral_seams_permitted_h3_capability_gated",
        "Phase 0 health contract",
    )?;
    verify_grocery_contracts(root)?;
    validate_health_contract_provenance(root)?;
    validate_grok_pattern_provenance(root)?;

    let review = field(inventory, "review", "Phase 0 inventory")?.object("Phase 0 review")?;
    let pending = validate_review(
        field(inventory, "review", "Phase 0 inventory")?,
        "Phase 0 review",
    )?;
    let review_status = required_string(review, "status", "Phase 0 review")?;
    if !pending && blockers != 0 {
        return Err("Phase 0 inventory cannot be approved while blockers remain".to_owned());
    }
    Ok(Phase0EvidenceReport {
        requirements: requirements.len(),
        blockers,
        review_status: review_status.to_owned(),
    })
}

/// Validate Phase 1 evidence while preserving the two deliberately unresolved
/// external boundaries. A structural pass is not an approval: blockers and the
/// independent review status remain explicit in the returned report.
pub fn verify_phase1_evidence(root: &Path) -> Result<Phase1EvidenceReport, String> {
    let document = read_json(
        root,
        "docs/release-evidence/rust-phase1/phase1-inventory.json",
    )?;
    let inventory = document.object("Phase 1 inventory")?;
    exact_keys(
        inventory,
        &[
            "schema_version",
            "evidence_date",
            "rust_lineage",
            "requirements",
            "external_gates",
            "review",
        ],
        "Phase 1 inventory",
    )?;
    expect_usize(inventory, "schema_version", 1, "Phase 1 inventory")?;
    required_nonempty_string(inventory, "evidence_date", "Phase 1 inventory")?;

    let lineage =
        field(inventory, "rust_lineage", "Phase 1 inventory")?.object("Phase 1 rust lineage")?;
    exact_keys(
        lineage,
        &[
            "repository",
            "branch",
            "code_commit_sha",
            "evidence_commit_sha",
        ],
        "Phase 1 rust lineage",
    )?;
    required_nonempty_string(lineage, "repository", "Phase 1 rust lineage")?;
    required_nonempty_string(lineage, "branch", "Phase 1 rust lineage")?;
    let code_sha = required_string(lineage, "code_commit_sha", "Phase 1 rust lineage")?;
    validate_git_sha(code_sha, "Phase 1 rust_lineage.code_commit_sha")?;
    if !matches!(
        field(lineage, "evidence_commit_sha", "Phase 1 rust lineage")?,
        Json::Null
    ) {
        validate_git_sha(
            required_string(lineage, "evidence_commit_sha", "Phase 1 rust lineage")?,
            "Phase 1 rust_lineage.evidence_commit_sha",
        )?;
    }

    let requirements =
        field(inventory, "requirements", "Phase 1 inventory")?.array("Phase 1 requirements")?;
    if requirements.len() != 10 {
        return Err("Phase 1 inventory must contain the ten approved requirements".to_owned());
    }
    let mut ids = BTreeSet::new();
    let mut blockers = 0;
    for (index, requirement) in requirements.iter().enumerate() {
        let context = format!("Phase 1 requirements[{index}]");
        let requirement = requirement.object(&context)?;
        exact_keys(
            requirement,
            &["id", "status", "evidence", "blocker"],
            &context,
        )?;
        let id = required_nonempty_string(requirement, "id", &context)?;
        if !ids.insert(id) {
            return Err(format!("duplicate Phase 1 requirement ID {id:?}"));
        }
        let evidence = field(requirement, "evidence", &context)?.array("evidence")?;
        if evidence.is_empty() {
            return Err(format!("{context}.evidence must not be empty"));
        }
        for value in evidence {
            required_path_exists(root, value.string("Phase 1 evidence")?, &context)?;
        }
        match required_string(requirement, "status", &context)? {
            "satisfied" => {
                if !matches!(field(requirement, "blocker", &context)?, Json::Null) {
                    return Err(format!("{context} satisfied requirement has a blocker"));
                }
            }
            "blocked" => {
                blockers += 1;
                required_nonempty_string(requirement, "blocker", &context)?;
            }
            status => return Err(format!("{context}.status has invalid value {status:?}")),
        }
    }

    let gates = field(inventory, "external_gates", "Phase 1 inventory")?
        .object("Phase 1 external gates")?;
    exact_keys(
        gates,
        &["grocery_phase_a_wire", "kroger_token_storage", "health"],
        "Phase 1 external gates",
    )?;
    let grocery = field(gates, "grocery_phase_a_wire", "Phase 1 external gates")?
        .object("Phase 1 Grocery gate")?;
    exact_keys(
        grocery,
        &[
            "status",
            "authoritative_contract_sha",
            "authoritative_contract_digest",
            "permitted_phase1_scope",
            "prohibited",
        ],
        "Phase 1 Grocery gate",
    )?;
    expect_string(
        grocery,
        "status",
        "deployment_and_canaries_required",
        "Phase 1 Grocery gate",
    )?;
    if !null_fields(
        grocery,
        &[
            "authoritative_contract_sha",
            "authoritative_contract_digest",
        ],
    )? {
        return Err("Phase 1 must not claim final Grocery wire provenance".to_owned());
    }
    expect_string(
        grocery,
        "permitted_phase1_scope",
        "generic_semantics_ports_and_id_only_cache",
        "Phase 1 Grocery gate",
    )?;
    let grocery_prohibited = field(grocery, "prohibited", "Phase 1 Grocery gate")?
        .array("Phase 1 Grocery prohibited list")?;
    for required in [
        "final_wire_dtos",
        "grocery_rest_calls",
        "grocery_tool_binding",
    ] {
        if !grocery_prohibited
            .iter()
            .any(|value| value.string("Grocery prohibition") == Ok(required))
        {
            return Err(format!("Phase 1 Grocery gate must prohibit {required}"));
        }
    }

    let kroger = field(gates, "kroger_token_storage", "Phase 1 external gates")?
        .object("Phase 1 Kroger gate")?;
    exact_keys(
        kroger,
        &[
            "status",
            "integration_key_contract_sha",
            "permitted_phase1_scope",
            "prohibited",
        ],
        "Phase 1 Kroger gate",
    )?;
    expect_string(
        kroger,
        "status",
        "security_d2_required",
        "Phase 1 Kroger gate",
    )?;
    if !matches!(
        field(
            kroger,
            "integration_key_contract_sha",
            "Phase 1 Kroger gate"
        )?,
        Json::Null
    ) {
        return Err("Phase 1 must not claim a Security D2 key contract".to_owned());
    }
    expect_string(
        kroger,
        "permitted_phase1_scope",
        "none",
        "Phase 1 Kroger gate",
    )?;

    let health = field(gates, "health", "Phase 1 external gates")?.object("Phase 1 Health gate")?;
    exact_keys(
        health,
        &[
            "status",
            "h1_h2_contract_sha",
            "h3_runtime_capability",
            "provider_token_custody",
        ],
        "Phase 1 Health gate",
    )?;
    expect_string(
        health,
        "status",
        "h1_h2_provider_neutral_seams_only",
        "Phase 1 Health gate",
    )?;
    validate_git_sha(
        required_string(health, "h1_h2_contract_sha", "Phase 1 Health gate")?,
        "Phase 1 Health h1_h2_contract_sha",
    )?;
    if field(health, "h3_runtime_capability", "Phase 1 Health gate")?
        .boolean("Phase 1 Health h3_runtime_capability")?
    {
        return Err("Phase 1 H3 runtime capability must remain false".to_owned());
    }
    expect_string(
        health,
        "provider_token_custody",
        "server_only",
        "Phase 1 Health gate",
    )?;

    let qualification = read_json(
        root,
        "docs/release-evidence/rust-phase1/qualification-evidence.json",
    )?;
    let qualification = qualification.object("Phase 1 qualification evidence")?;
    expect_string(
        qualification,
        "code_commit_sha",
        code_sha,
        "Phase 1 qualification evidence",
    )?;
    let hosted = field(qualification, "hosted", "Phase 1 qualification evidence")?
        .object("Phase 1 hosted evidence")?;
    let hosted_status = required_string(hosted, "status", "Phase 1 hosted evidence")?;
    if !matches!(hosted_status, "pending" | "passed") {
        return Err("Phase 1 hosted status must be pending or passed".to_owned());
    }
    let privacy = field(qualification, "privacy", "Phase 1 qualification evidence")?
        .object("Phase 1 privacy evidence")?;
    for name in [
        "item_cache_contains_sensitive_labels",
        "provider_oauth_token_model_present",
    ] {
        if field(privacy, name, "Phase 1 privacy evidence")?
            .boolean(&format!("Phase 1 privacy evidence.{name}"))?
        {
            return Err(format!("Phase 1 privacy evidence.{name} must be false"));
        }
    }

    let review = field(inventory, "review", "Phase 1 inventory")?.object("Phase 1 review")?;
    let pending = validate_review(
        field(inventory, "review", "Phase 1 inventory")?,
        "Phase 1 review",
    )?;
    let review_status = required_string(review, "status", "Phase 1 review")?;
    if !pending && (blockers != 0 || hosted_status != "passed") {
        return Err(
            "Phase 1 cannot be approved while blockers or hosted evidence remain".to_owned(),
        );
    }

    Ok(Phase1EvidenceReport {
        requirements: requirements.len(),
        blockers,
        review_status: review_status.to_owned(),
        hosted_status: hosted_status.to_owned(),
    })
}

fn validate_health_contract_provenance(root: &Path) -> Result<(), String> {
    let document = read_json(root, "fixtures/contracts/health-contract-provenance.json")?;
    let provenance = document.object("health contract provenance")?;
    exact_keys(
        provenance,
        &["schema_version", "source_repository", "contracts"],
        "health contract provenance",
    )?;
    expect_usize(
        provenance,
        "schema_version",
        1,
        "health contract provenance",
    )?;
    required_nonempty_string(
        provenance,
        "source_repository",
        "health contract provenance",
    )?;
    let contracts = field(provenance, "contracts", "health contract provenance")?
        .array("health contract provenance.contracts")?;
    if contracts.len() != 2 {
        return Err("health contract provenance must contain H1/H2 and H3".to_owned());
    }
    let mut names = BTreeSet::new();
    for (index, contract) in contracts.iter().enumerate() {
        let context = format!("health contract provenance.contracts[{index}]");
        let contract = contract.object(&context)?;
        exact_keys(
            contract,
            &[
                "name",
                "source_pr",
                "source_commit",
                "source_pr_head",
                "sources",
                "target",
                "freeze_kind",
            ],
            &context,
        )?;
        let name = required_nonempty_string(contract, "name", &context)?;
        if !names.insert(name) {
            return Err(format!(
                "duplicate health contract provenance name {name:?}"
            ));
        }
        match name {
            "health_h1_h2" => expect_usize(contract, "source_pr", 79, &context)?,
            "health_h3_daily_sync" => expect_usize(contract, "source_pr", 96, &context)?,
            _ => return Err(format!("unknown health contract provenance name {name:?}")),
        }
        for field_name in ["source_commit", "source_pr_head"] {
            validate_git_sha(
                required_string(contract, field_name, &context)?,
                &format!("{context}.{field_name}"),
            )?;
        }
        let sources = field(contract, "sources", &context)?.array(&format!("{context}.sources"))?;
        if sources.is_empty() {
            return Err(format!("{context}.sources must not be empty"));
        }
        for (source_index, source) in sources.iter().enumerate() {
            let source_context = format!("{context}.sources[{source_index}]");
            let source = source.object(&source_context)?;
            exact_keys(source, &["path", "sha256"], &source_context)?;
            required_nonempty_string(source, "path", &source_context)?;
            validate_sha256(
                required_string(source, "sha256", &source_context)?,
                &format!("{source_context}.sha256"),
            )?;
        }
        let target = field(contract, "target", &context)?.object(&format!("{context}.target"))?;
        exact_keys(target, &["path", "sha256"], &format!("{context}.target"))?;
        let target_path = safe_relative_path(required_string(target, "path", &context)?)?;
        let target_sha = required_string(target, "sha256", &context)?;
        validate_sha256(target_sha, &format!("{context}.target.sha256"))?;
        expect_hash(&read_bytes(root, &target_path)?, target_sha, &target_path)?;
        match required_string(contract, "freeze_kind", &context)? {
            "language_neutral_projection" => {}
            "exact_copy" => {
                let first_source = sources[0].object(&format!("{context}.sources[0]"))?;
                if required_string(first_source, "sha256", &context)? != target_sha {
                    return Err(format!(
                        "{context} exact copy source and target hashes differ"
                    ));
                }
            }
            value => return Err(format!("{context}.freeze_kind has invalid value {value:?}")),
        }
    }
    Ok(())
}

fn validate_grok_pattern_provenance(root: &Path) -> Result<(), String> {
    let document = read_json(
        root,
        "docs/release-evidence/rust-phase0/grok-pattern-origin.json",
    )?;
    let provenance = document.object("Grok pattern provenance")?;
    exact_keys(
        provenance,
        &[
            "schema_version",
            "source_repository",
            "source_commit",
            "source_license",
            "license_path",
            "license_sha256",
            "copy_policy",
            "origins",
            "review",
        ],
        "Grok pattern provenance",
    )?;
    expect_usize(provenance, "schema_version", 1, "Grok pattern provenance")?;
    expect_string(
        provenance,
        "source_repository",
        "https://github.com/xai-org/grok-build.git",
        "Grok pattern provenance",
    )?;
    expect_string(
        provenance,
        "source_commit",
        "b189869b7755d2b482969acf6c92da3ecfeffd36",
        "Grok pattern provenance",
    )?;
    expect_string(
        provenance,
        "source_license",
        "Apache-2.0",
        "Grok pattern provenance",
    )?;
    expect_string(
        provenance,
        "license_path",
        "LICENSE",
        "Grok pattern provenance",
    )?;
    validate_sha256(
        required_string(provenance, "license_sha256", "Grok pattern provenance")?,
        "Grok pattern provenance.license_sha256",
    )?;
    expect_string(
        provenance,
        "copy_policy",
        "pattern_only_no_source_bytes",
        "Grok pattern provenance",
    )?;
    let origins = field(provenance, "origins", "Grok pattern provenance")?
        .array("Grok pattern provenance.origins")?;
    if origins.is_empty() {
        return Err("Grok pattern provenance origins must not be empty".to_owned());
    }
    for (index, origin) in origins.iter().enumerate() {
        let context = format!("Grok pattern provenance.origins[{index}]");
        let origin = origin.object(&context)?;
        exact_keys(
            origin,
            &[
                "pattern",
                "source_paths",
                "heyfood_paths",
                "disposition",
                "copied_bytes",
            ],
            &context,
        )?;
        for field_name in ["pattern", "disposition"] {
            required_nonempty_string(origin, field_name, &context)?;
        }
        let sources =
            field(origin, "source_paths", &context)?.array(&format!("{context}.source_paths"))?;
        if sources.is_empty() {
            return Err(format!("{context}.source_paths must not be empty"));
        }
        for (source_index, source) in sources.iter().enumerate() {
            let source_context = format!("{context}.source_paths[{source_index}]");
            let source = source.object(&source_context)?;
            exact_keys(source, &["path", "sha256"], &source_context)?;
            required_nonempty_string(source, "path", &source_context)?;
            validate_sha256(
                required_string(source, "sha256", &source_context)?,
                &format!("{source_context}.sha256"),
            )?;
        }
        let targets =
            field(origin, "heyfood_paths", &context)?.array(&format!("{context}.heyfood_paths"))?;
        if targets.is_empty() {
            return Err(format!("{context}.heyfood_paths must not be empty"));
        }
        for target in targets {
            required_path_exists(root, target.string(&context)?, &context)?;
        }
        if field(origin, "copied_bytes", &context)?.boolean(&context)? {
            return Err(format!("{context} must not claim copied Grok source bytes"));
        }
    }
    validate_review(
        field(provenance, "review", "Grok pattern provenance")?,
        "Grok pattern provenance.review",
    )?;
    Ok(())
}

fn required_path_exists(root: &Path, value: &str, context: &str) -> Result<(), String> {
    let path = safe_relative_path(value)?;
    if !root.join(&path).exists() {
        return Err(format!(
            "{context} evidence does not exist: {}",
            path.display()
        ));
    }
    Ok(())
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
        if package.version.to_string() != "0.4.0" {
            return Err(format!(
                "{} has internal version {}; expected exact workspace version 0.4.0",
                package.name, package.version
            ));
        }
        let manifest_parent = package
            .manifest_path
            .parent()
            .ok_or_else(|| format!("{} manifest has no parent", package.name))?;
        let crates_root = metadata.workspace_root.join("crates");
        if manifest_parent.parent() != Some(crates_root.as_ref()) {
            return Err(format!(
                "{} is outside the direct workspace crates/ containment boundary: {}",
                package.name, package.manifest_path
            ));
        }
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
        for dependency in package
            .dependencies
            .iter()
            .filter(|dependency| workspace_names.contains(dependency.name.as_str()))
        {
            if dependency.req.to_string() != "=0.4.0" {
                return Err(format!(
                    "{} -> {} must use exact internal version =0.4.0; found {}",
                    package.name, dependency.name, dependency.req
                ));
            }
            if dependency.source.is_some() {
                return Err(format!(
                    "{} -> {} must be a workspace-contained path source",
                    package.name, dependency.name
                ));
            }
            let path = dependency.path.as_ref().ok_or_else(|| {
                format!(
                    "{} -> {} is missing its required internal path source",
                    package.name, dependency.name
                )
            })?;
            if path.parent() != Some(crates_root.as_ref())
                || path.file_name() != Some(dependency.name.as_str())
            {
                return Err(format!(
                    "{} -> {} escapes or aliases the workspace crates boundary: {path}",
                    package.name, dependency.name
                ));
            }
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
    expect_usize(provenance, "export_tool_version", 2, "dietary provenance")?;
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
    expect_usize(provenance, "export_tool_version", 2, "brand provenance")?;
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

fn require_exact_endpoint_rows(
    candidate_rows: &[Json],
    frozen_rows: &[Json],
    context: &str,
) -> Result<(), String> {
    let candidate_keys = endpoint_keys(candidate_rows)?;
    let frozen_keys = endpoint_keys(frozen_rows)?;
    if !frozen_keys.is_subset(&candidate_keys) {
        return Err(set_difference(
            &format!("{context} frozen endpoint coverage"),
            &frozen_keys,
            &candidate_keys,
        ));
    }
    for frozen_row in frozen_rows {
        let frozen_row = frozen_row.object("frozen endpoint")?;
        let method = required_string(frozen_row, "method", "frozen endpoint")?;
        let endpoint = required_string(frozen_row, "endpoint", "frozen endpoint")?;
        let candidate = candidate_rows.iter().find(|row| {
            row.object(context).is_ok_and(|row| {
                required_string(row, "method", context) == Ok(method)
                    && required_string(row, "endpoint", context) == Ok(endpoint)
            })
        });
        if candidate.and_then(|row| row.object(context).ok()) != Some(frozen_row) {
            return Err(format!(
                "{context} changed frozen compatibility row {method} {endpoint}"
            ));
        }
    }
    Ok(())
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
    let actual = sha256::repository_text_digest_hex(bytes).map_err(|error| {
        format!(
            "cannot hash non-UTF-8 repository text {}: {error}",
            path.display()
        )
    })?;
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
        FROZEN_COMPATIBILITY_DIGEST, FROZEN_COMPATIBILITY_SHA, FROZEN_COMPATIBILITY_TREE,
        import_grocery_contracts, validate_dependency_dag, validate_grok_pattern_provenance,
        validate_health_contract_provenance, verify_assets, verify_assets_approved,
        verify_grocery_contracts, verify_migration_ledger, verify_phase0_evidence,
        verify_phase1_evidence, verify_stable_contracts,
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

    fn convert_to_crlf(relative: &str, destination_root: &std::path::Path) {
        copy(relative, destination_root);
        let destination = destination_root.join(relative);
        let text = fs::read_to_string(&destination).unwrap();
        fs::write(
            destination,
            text.replace("\r\n", "\n").replace('\n', "\r\n"),
        )
        .unwrap();
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
        assert_eq!(verify_stable_contracts(&root()).unwrap().endpoints, 27);
        assert_eq!(verify_grocery_contracts(&root()).unwrap().contracts, 3);
        assert_eq!(verify_assets(&root()).unwrap().pending_reviews, 0);
        verify_assets_approved(&root()).expect("specialist-approved assets must validate");
        let phase0 = verify_phase0_evidence(&root()).expect("Phase 0 inventory must validate");
        assert_eq!(phase0.blockers, 0);
        assert_eq!(phase0.review_status, "approved");
    }

    #[test]
    fn checked_in_phase1_evidence_preserves_external_gates() {
        let phase1 = verify_phase1_evidence(&root()).expect("Phase 1 evidence must validate");
        assert_eq!(phase1.requirements, 10);
        assert_eq!(phase1.blockers, 2);
        assert_eq!(phase1.hosted_status, "pending");
        assert_eq!(phase1.review_status, "pending");
    }

    #[test]
    fn grocery_contract_validator_rejects_target_corruption() {
        let scratch = scratch("grocery-contract-corruption");
        for path in [
            "fixtures/contracts/grocery-backend/provenance.json",
            "fixtures/contracts/grocery-backend/c3-confirmation-contract.json",
            "fixtures/contracts/grocery-backend/c4-application-capabilities-contract.json",
            "fixtures/contracts/grocery-backend/c4-scopes-contract.json",
        ] {
            copy(path, &scratch);
        }
        verify_grocery_contracts(&scratch)
            .expect("checked-in grocery contract provenance must validate");
        let contract =
            scratch.join("fixtures/contracts/grocery-backend/c3-confirmation-contract.json");
        let mut corrupted = fs::read(&contract).unwrap();
        corrupted.extend_from_slice(b"\ncorruption\n");
        fs::write(contract, corrupted).unwrap();
        assert!(verify_grocery_contracts(&scratch).is_err());
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn grocery_contract_validator_accepts_windows_checkout_line_endings() {
        let scratch = scratch("grocery-contract-crlf");
        for path in [
            "fixtures/contracts/grocery-backend/provenance.json",
            "fixtures/contracts/grocery-backend/c3-confirmation-contract.json",
            "fixtures/contracts/grocery-backend/c4-application-capabilities-contract.json",
            "fixtures/contracts/grocery-backend/c4-scopes-contract.json",
        ] {
            convert_to_crlf(path, &scratch);
        }
        verify_grocery_contracts(&scratch)
            .expect("CRLF checkout must preserve grocery contract digests");
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn grocery_contract_import_failure_does_not_modify_targets() {
        let scratch = scratch("grocery-contract-import-atomicity");
        for path in [
            "fixtures/contracts/grocery-backend/provenance.json",
            "fixtures/contracts/grocery-backend/c3-confirmation-contract.json",
            "fixtures/contracts/grocery-backend/c4-application-capabilities-contract.json",
            "fixtures/contracts/grocery-backend/c4-scopes-contract.json",
        ] {
            copy(path, &scratch);
        }
        let target =
            scratch.join("fixtures/contracts/grocery-backend/c3-confirmation-contract.json");
        let before = fs::read(&target).unwrap();
        assert!(import_grocery_contracts(&scratch, &scratch.join("missing-source")).is_err());
        assert_eq!(fs::read(target).unwrap(), before);
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn health_contract_validator_rejects_target_corruption() {
        let scratch = scratch("health-contract-corruption");
        for path in [
            "fixtures/contracts/health-contract-provenance.json",
            "fixtures/contracts/health-h1h2.v1.json",
            "fixtures/contracts/health-h3-daily-sync.v1.json",
        ] {
            copy(path, &scratch);
        }
        validate_health_contract_provenance(&scratch)
            .expect("checked-in health contract provenance must validate");
        let contract = scratch.join("fixtures/contracts/health-h1h2.v1.json");
        let corrupted = fs::read_to_string(&contract).unwrap().replacen(
            "\"oura\"",
            "\"corrupted-provider\"",
            1,
        );
        fs::write(contract, corrupted).unwrap();
        assert!(validate_health_contract_provenance(&scratch).is_err());
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn grok_pattern_validator_rejects_copied_source_claim() {
        let scratch = scratch("grok-pattern-corruption");
        for path in [
            "docs/release-evidence/rust-phase0/grok-pattern-origin.json",
            "crates/heyfood-bin/src/main.rs",
            "crates/heyfood-bin/src/lib.rs",
            "crates/heyfood-tui/src/terminal.rs",
            "crates/heyfood-tui/src/loop_driver.rs",
            "crates/heyfood-bin/tests/phase0_qualification.rs",
            "crates/heyfood-application/src/run_turn.rs",
        ] {
            copy(path, &scratch);
        }
        validate_grok_pattern_provenance(&scratch)
            .expect("checked-in Grok pattern provenance must validate");
        let provenance = scratch.join("docs/release-evidence/rust-phase0/grok-pattern-origin.json");
        let corrupted = fs::read_to_string(&provenance).unwrap().replacen(
            "\"copied_bytes\": false",
            "\"copied_bytes\": true",
            1,
        );
        fs::write(provenance, corrupted).unwrap();
        assert!(validate_grok_pattern_provenance(&scratch).is_err());
        fs::remove_dir_all(scratch).unwrap();
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
    fn ledger_validator_accepts_windows_checkout_line_endings() {
        let scratch = scratch("ledger-crlf");
        for path in [
            "schemas/migration-ledger.v1.schema.json",
            "tests/migration/python-test-ledger.json",
            "tests/migration/python-node-ids.txt",
            "tests/migration/non-pytest-invariants.json",
        ] {
            convert_to_crlf(path, &scratch);
        }
        verify_migration_ledger(&scratch).expect("CRLF checkout must preserve ledger digests");
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
        let contract = scratch.join("fixtures/contracts/called-endpoints.json");
        let corrupted = fs::read_to_string(&contract).unwrap().replacen(
            FROZEN_COMPATIBILITY_DIGEST,
            "01986e9dbfdfd415f6da5183321e1614ea15c1f3be12af7f32a3e5578423e1e9",
            1,
        );
        fs::write(contract, corrupted).unwrap();
        assert!(verify_stable_contracts(&scratch).is_err());
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn contract_validator_rejects_baseline_identity_corruption() {
        for (label, original, corrupted) in [
            (
                "sha",
                FROZEN_COMPATIBILITY_SHA,
                "03494a57468dac83b4904ce6c390e36926f5c6fe",
            ),
            (
                "tree",
                FROZEN_COMPATIBILITY_TREE,
                "0c265cd9ae0623442dd8eba1f6f4388c4ebf5adf",
            ),
        ] {
            let scratch = scratch(&format!("contract-{label}-corruption"));
            for path in [
                "fixtures/contracts/called-endpoints.json",
                "tests/fixtures/called_endpoints.json",
            ] {
                copy(path, &scratch);
            }
            let contract = scratch.join("fixtures/contracts/called-endpoints.json");
            let corrupted = fs::read_to_string(&contract)
                .unwrap()
                .replacen(original, corrupted, 1);
            fs::write(contract, corrupted).unwrap();
            assert!(verify_stable_contracts(&scratch).is_err());
            fs::remove_dir_all(scratch).unwrap();
        }
    }

    #[test]
    fn contract_validator_rejects_live_frozen_row_mutation() {
        let scratch = scratch("contract-live-row-corruption");
        for path in [
            "fixtures/contracts/called-endpoints.json",
            "tests/fixtures/called_endpoints.json",
        ] {
            copy(path, &scratch);
        }
        let fixture = scratch.join("tests/fixtures/called_endpoints.json");
        let corrupted = fs::read_to_string(&fixture).unwrap().replacen(
            "/v1/auth/capabilities",
            "/v1/auth/capabilities-v2",
            1,
        );
        fs::write(fixture, corrupted).unwrap();
        assert!(verify_stable_contracts(&scratch).is_err());
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn contract_validator_rejects_stable_frozen_row_mutation() {
        let scratch = scratch("contract-stable-row-corruption");
        for path in [
            "fixtures/contracts/called-endpoints.json",
            "tests/fixtures/called_endpoints.json",
        ] {
            copy(path, &scratch);
        }
        let contract = scratch.join("fixtures/contracts/called-endpoints.json");
        let corrupted = fs::read_to_string(&contract).unwrap().replacen(
            "/v1/auth/capabilities",
            "/v1/auth/capabilities-v2",
            1,
        );
        fs::write(contract, corrupted).unwrap();
        assert!(verify_stable_contracts(&scratch).is_err());
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn contract_validator_accepts_windows_checkout_line_endings() {
        let scratch = scratch("contract-crlf");
        for path in [
            "fixtures/contracts/called-endpoints.json",
            "tests/fixtures/called_endpoints.json",
        ] {
            convert_to_crlf(path, &scratch);
        }
        verify_stable_contracts(&scratch)
            .expect("CRLF checkout must preserve frozen compatibility semantics");
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

    #[test]
    fn asset_validator_accepts_windows_checkout_line_endings() {
        let scratch = scratch("asset-crlf");
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
            convert_to_crlf(path, &scratch);
        }
        verify_assets(&scratch).expect("CRLF checkout must preserve asset digests");
        fs::remove_dir_all(scratch).unwrap();
    }
}
