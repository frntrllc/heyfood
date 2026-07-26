//! Post-release installed-artifact UX evidence evaluation.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostReleaseEvalSummary {
    pub passed: bool,
    pub score: u64,
    pub threshold: u64,
    pub findings: usize,
}

pub fn evaluate(
    evidence_directory: &Path,
    rubric_path: &Path,
    output_path: &Path,
) -> Result<PostReleaseEvalSummary, String> {
    let rubric = read_json(rubric_path, "post-release UX rubric")?;
    let evidence_path = evidence_directory.join("installed-core-matrix.json");
    let evidence = read_json(&evidence_path, "installed-artifact evidence")?;

    require_u64(&rubric, "schema_version", "post-release UX rubric", 1)?;
    require_u64(
        &evidence,
        "schema_version",
        "installed-artifact evidence",
        2,
    )?;
    require_string(
        &evidence,
        "qualification",
        "installed-artifact evidence",
        "installed-artifact-core-matrix",
    )?;

    let rubric_id = string_field(&rubric, "id", "post-release UX rubric")?;
    let minimum_version = string_field(&rubric, "minimum_release", "post-release UX rubric")?;
    let threshold = u64_field(&rubric, "pass_threshold", "post-release UX rubric")?;
    if threshold > 100 {
        return Err("post-release UX rubric pass_threshold cannot exceed 100".to_owned());
    }

    let archive = object_field(&evidence, "archive", "installed-artifact evidence")?;
    let version = string_field(archive, "version", "installed-artifact archive")?;
    let target = string_field(archive, "target", "installed-artifact archive")?;
    if compare_versions(version, minimum_version)? == std::cmp::Ordering::Less {
        return Err(format!(
            "release {version} predates rubric minimum {minimum_version}"
        ));
    }

    let groups = array_field(&evidence, "core_matrix", "installed-artifact evidence")?;
    let categories = array_field(&rubric, "categories", "post-release UX rubric")?;
    let mut observed_category_ids = BTreeSet::new();
    let mut score = 0_u64;
    let mut total_weight = 0_u64;
    let mut category_results = Vec::with_capacity(categories.len());
    let mut findings = Vec::new();

    for category in categories {
        let context = "post-release UX rubric category";
        let id = string_field(category, "id", context)?;
        if !observed_category_ids.insert(id.to_owned()) {
            return Err(format!("duplicate post-release UX category `{id}`"));
        }
        let objective = string_field(category, "objective", context)?;
        let evidence_group = string_field(category, "evidence_group", context)?;
        let severity = string_field(category, "severity", context)?;
        if !matches!(severity, "P0" | "P1" | "P2" | "P3") {
            return Err(format!(
                "category `{id}` has unsupported severity `{severity}`"
            ));
        }
        let weight = u64_field(category, "weight", context)?;
        if weight == 0 {
            return Err(format!("category `{id}` weight must be positive"));
        }
        total_weight = total_weight
            .checked_add(weight)
            .ok_or_else(|| "post-release UX category weights overflowed".to_owned())?;

        let accepted_statuses = string_set(
            array_field(category, "accepted_statuses", context)?,
            &format!("category `{id}` accepted_statuses"),
        )?;
        if accepted_statuses.is_empty() {
            return Err(format!("category `{id}` must accept at least one status"));
        }
        let required_assertions = string_set(
            array_field(category, "required_assertions", context)?,
            &format!("category `{id}` required_assertions"),
        )?;

        let matching_group = groups.iter().find(|group| {
            group
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|group_id| group_id == evidence_group)
        });
        let (observed_status, missing_assertions, passed) = match matching_group {
            Some(group) => {
                let observed_status = string_field(group, "status", "core matrix group")?;
                let observed_assertions = string_set(
                    array_field(group, "assertions", "core matrix group")?,
                    &format!("core matrix group `{evidence_group}` assertions"),
                )?;
                let missing = required_assertions
                    .difference(&observed_assertions)
                    .cloned()
                    .collect::<Vec<_>>();
                let passed = accepted_statuses.contains(observed_status) && missing.is_empty();
                (observed_status.to_owned(), missing, passed)
            }
            None => (
                "missing".to_owned(),
                required_assertions.iter().cloned().collect(),
                false,
            ),
        };

        if passed {
            score += weight;
        } else {
            findings.push(json!({
                "fingerprint": format!("{rubric_id}:{id}"),
                "severity": severity,
                "category": id,
                "summary": format!("{objective} failed for {target}"),
                "observed_status": observed_status,
                "missing_assertions": missing_assertions
            }));
        }
        category_results.push(json!({
            "id": id,
            "objective": objective,
            "severity": severity,
            "weight": weight,
            "status": if passed { "passed" } else { "failed" },
            "evidence_group": evidence_group,
            "observed_status": observed_status,
            "missing_assertions": missing_assertions
        }));
    }

    if total_weight != 100 {
        return Err(format!(
            "post-release UX category weights must total 100, observed {total_weight}"
        ));
    }

    let passed = score >= threshold && findings.is_empty();
    let limitations = rubric
        .get("limitations")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let roadmap = rubric
        .get("roadmap_observations")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let report = json!({
        "schema_version": 1,
        "evaluation": rubric_id,
        "status": if passed { "passed" } else { "failed" },
        "score": score,
        "maximum_score": 100,
        "pass_threshold": threshold,
        "release": {
            "version": version,
            "target": target,
            "archive_sha256": archive.get("sha256").and_then(Value::as_str)
        },
        "source": {
            "installed_artifact": true,
            "real_pty": true,
            "synthetic_backend": evidence
                .pointer("/environment/synthetic_backend")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            "evidence_file": "installed-core-matrix.json"
        },
        "categories": category_results,
        "findings": findings,
        "roadmap_observations": roadmap,
        "limitations": limitations
    });
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create evaluation output directory: {error}"))?;
    }
    fs::write(
        output_path,
        serde_json::to_vec_pretty(&report)
            .map_err(|error| format!("encode post-release UX report: {error}"))?,
    )
    .map_err(|error| format!("write {}: {error}", output_path.display()))?;

    Ok(PostReleaseEvalSummary {
        passed,
        score,
        threshold,
        findings: report["findings"].as_array().map_or(0, Vec::len),
    })
}

fn read_json(path: &Path, context: &str) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("decode {context}: {error}"))
}

fn object_field<'a>(value: &'a Value, field: &str, context: &str) -> Result<&'a Value, String> {
    value
        .get(field)
        .filter(|item| item.is_object())
        .ok_or_else(|| format!("{context}.{field} must be an object"))
}

fn array_field<'a>(value: &'a Value, field: &str, context: &str) -> Result<&'a [Value], String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{context}.{field} must be an array"))
}

fn string_field<'a>(value: &'a Value, field: &str, context: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context}.{field} must be a string"))
}

fn u64_field(value: &Value, field: &str, context: &str) -> Result<u64, String> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{context}.{field} must be a non-negative integer"))
}

fn require_u64(value: &Value, field: &str, context: &str, expected: u64) -> Result<(), String> {
    let observed = u64_field(value, field, context)?;
    if observed == expected {
        Ok(())
    } else {
        Err(format!(
            "{context}.{field} must be {expected}, observed {observed}"
        ))
    }
}

fn require_string(value: &Value, field: &str, context: &str, expected: &str) -> Result<(), String> {
    let observed = string_field(value, field, context)?;
    if observed == expected {
        Ok(())
    } else {
        Err(format!(
            "{context}.{field} must be `{expected}`, observed `{observed}`"
        ))
    }
}

fn string_set(values: &[Value], context: &str) -> Result<BTreeSet<String>, String> {
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{context} entries must be strings"))
        })
        .collect()
}

fn compare_versions(left: &str, right: &str) -> Result<std::cmp::Ordering, String> {
    Ok(parse_version(left)?.cmp(&parse_version(right)?))
}

fn parse_version(value: &str) -> Result<(u64, u64, u64), String> {
    let mut parts = value.split('.');
    let parse_part = |part: Option<&str>| {
        part.ok_or_else(|| format!("version `{value}` must use MAJOR.MINOR.PATCH"))?
            .parse::<u64>()
            .map_err(|_| format!("version `{value}` must use MAJOR.MINOR.PATCH"))
    };
    let parsed = (
        parse_part(parts.next())?,
        parse_part(parts.next())?,
        parse_part(parts.next())?,
    );
    if parts.next().is_some() {
        return Err(format!("version `{value}` must use MAJOR.MINOR.PATCH"));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::evaluate;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("heyfood-post-release-{label}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_fixture(root: &std::path::Path, observed_status: &str) -> (PathBuf, PathBuf) {
        let evidence = root.join("evidence");
        fs::create_dir_all(&evidence).unwrap();
        fs::write(
            evidence.join("installed-core-matrix.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": 2,
                "qualification": "installed-artifact-core-matrix",
                "archive": {
                    "version": "0.5.0",
                    "target": "aarch64-apple-darwin",
                    "sha256": "abc"
                },
                "environment": {"synthetic_backend": true},
                "core_matrix": [{
                    "id": "clean-user",
                    "status": observed_status,
                    "assertions": ["registration_executed"]
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let rubric = root.join("rubric.json");
        fs::write(
            &rubric,
            serde_json::to_vec_pretty(&json!({
                "schema_version": 1,
                "id": "test-rubric-v1",
                "minimum_release": "0.5.0",
                "pass_threshold": 100,
                "categories": [{
                    "id": "first-run",
                    "objective": "First run works",
                    "severity": "P0",
                    "weight": 100,
                    "evidence_group": "clean-user",
                    "accepted_statuses": ["passed"],
                    "required_assertions": ["registration_executed"]
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        (evidence, rubric)
    }

    #[test]
    fn passing_evidence_produces_a_full_score() {
        let root = scratch("pass");
        let (evidence, rubric) = write_fixture(&root, "passed");
        let output = root.join("report.json");
        let result = evaluate(&evidence, &rubric, &output).unwrap();
        assert!(result.passed);
        assert_eq!(result.score, 100);
        assert_eq!(result.findings, 0);
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(report["status"], "passed");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_evidence_produces_a_stable_fingerprint() {
        let root = scratch("failure");
        let (evidence, rubric) = write_fixture(&root, "failed");
        let output = root.join("report.json");
        let result = evaluate(&evidence, &rubric, &output).unwrap();
        assert!(!result.passed);
        assert_eq!(result.score, 0);
        assert_eq!(result.findings, 1);
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(
            report["findings"][0]["fingerprint"],
            "test-rubric-v1:first-run"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
