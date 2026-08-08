//! Provider-neutral human presentation for the deployed household evaluation
//! contract.
//!
//! The backend response remains the machine contract. This module deliberately
//! projects only reviewed human fields: restaurant and item names, aggregate
//! safety, member labels, summaries, and conflict explanations. Stable IDs,
//! snapshot hashes, producer metadata, tool names, and raw JSON never cross the
//! presentation boundary.

use std::fmt;

use heyfood_core::{
    AnnotationDisposition, DietAlignment, EvaluateMenuResponse, EvaluationMemberId,
    EvaluationScope, MemberAnnotation, SafetyStatus, terminal_safe_text,
};
use serde_json::{Value, json};

const DEFAULT_PRESENTATION_WIDTH: usize = 80;
const MIN_PRESENTATION_WIDTH: usize = 20;
const MAX_NESTING_DEPTH: usize = 5;

/// Privacy-safe human copy for a response that looks like a household
/// evaluation but fails the reviewed typed contract.
pub const UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE: &str = "hey.food returned household guidance this version can’t display safely. Update heyfood, or ask about a specific item.";

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct HouseholdEvaluationPresentationError;

impl fmt::Debug for HouseholdEvaluationPresentationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HouseholdEvaluationPresentationError")
    }
}

impl fmt::Display for HouseholdEvaluationPresentationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE)
    }
}

impl std::error::Error for HouseholdEvaluationPresentationError {}

/// Render an additive household evaluation block at a stable default width.
///
/// `Ok(None)` preserves the pre-household presentation for non-evaluation
/// results, legacy results without the additive household block, and a valid
/// owner-only result. A likely but invalid household evaluation returns the
/// privacy-safe typed error instead of falling back to raw protocol text.
pub fn render_household_evaluation(
    document: &Value,
) -> Result<Option<String>, HouseholdEvaluationPresentationError> {
    render_household_evaluation_at_width(document, DEFAULT_PRESENTATION_WIDTH)
}

/// Render an additive household evaluation block using the available data
/// width. The line wrapper is deterministic and never inspects terminal state.
pub fn render_household_evaluation_at_width(
    document: &Value,
    width: usize,
) -> Result<Option<String>, HouseholdEvaluationPresentationError> {
    let Some(candidate) = household_evaluation_document(document) else {
        return Ok(None);
    };
    let mut typed_candidate = candidate.clone();
    normalize_owner_only_missing_labels(&mut typed_candidate);
    let evaluation = EvaluateMenuResponse::parse_value(typed_candidate)
        .map_err(|_| HouseholdEvaluationPresentationError)?;
    let Some(household) = evaluation.household.as_ref() else {
        return Ok(None);
    };
    if household.member_count == 1
        && household.members[0].member_id.is_self()
        && matches!(
            household.effective_scope,
            EvaluationScope::Self_ | EvaluationScope::Everyone
        )
    {
        return Ok(None);
    }
    if evaluation.items.iter().any(|item| {
        item.member_annotations.iter().any(|annotation| {
            private_identifier_shaped(annotation.label.as_str())
                || household
                    .members
                    .iter()
                    .any(|member| annotation.label.as_str() == member.member_id.as_str())
        })
    }) {
        return Err(HouseholdEvaluationPresentationError);
    }

    let width = width.max(MIN_PRESENTATION_WIDTH);
    let mut output = String::new();
    push_wrapped(
        &mut output,
        "",
        "",
        &format!(
            "Household evaluation at {}",
            inline_text(&evaluation.restaurant_name)
        ),
        width,
    );
    if evaluation.items.is_empty() {
        push_wrapped(
            &mut output,
            "",
            "",
            "No menu items were available to evaluate for this household.",
            width,
        );
        let output = output.trim_end().to_owned();
        return (household_output_is_private_safe(household, &output))
            .then_some(Some(output))
            .ok_or(HouseholdEvaluationPresentationError);
    }

    for item in &evaluation.items {
        push_wrapped(
            &mut output,
            "• ",
            "  ",
            &inline_text(&item.item_name),
            width,
        );
        let aggregate = explanation(&item.summary, item.status);
        push_wrapped(
            &mut output,
            "  ",
            "    ",
            &format!(
                "Household result: {} — {aggregate}",
                status_label(item.status)
            ),
            width,
        );
        for annotation in &item.member_annotations {
            append_member_annotation(&mut output, annotation, width);
        }
    }

    let output = output.trim_end().to_owned();
    if !household_output_is_private_safe(household, &output) {
        return Err(HouseholdEvaluationPresentationError);
    }
    Ok(Some(output))
}

/// Select a household-evaluation response from deployed agent result envelopes
/// without depending on a provider or tool name.
#[must_use]
pub fn household_evaluation_document(document: &Value) -> Option<&Value> {
    select_evaluation_document(document, 0)
}

fn select_evaluation_document(document: &Value, depth: usize) -> Option<&Value> {
    if looks_like_evaluate_menu(document) {
        return Some(document);
    }
    if depth >= MAX_NESTING_DEPTH {
        return None;
    }
    let object = document.as_object()?;
    for key in [
        "structured",
        "structured_content",
        "structuredContent",
        "result",
    ] {
        if let Some(candidate) = object
            .get(key)
            .and_then(|value| select_evaluation_document(value, depth + 1))
        {
            return Some(candidate);
        }
    }
    None
}

fn looks_like_evaluate_menu(document: &Value) -> bool {
    let Some(object) = document.as_object() else {
        return false;
    };
    let complete = [
        "restaurant_id",
        "restaurant_name",
        "items",
        "generally_safer",
        "risky",
        "avoid",
        "unmatched",
        "household",
    ]
    .into_iter()
    .all(|field| object.contains_key(field));
    if complete {
        return true;
    }

    object.contains_key("household")
        || object
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.get("member_annotations").is_some())
            })
}

fn normalize_owner_only_missing_labels(candidate: &mut Value) {
    let owner_only = candidate
        .get("household")
        .and_then(Value::as_object)
        .is_some_and(|household| {
            household.get("member_count").and_then(Value::as_u64) == Some(1)
                && matches!(
                    household.get("effective_scope").and_then(Value::as_str),
                    Some("_self" | "everyone")
                )
                && household
                    .get("members")
                    .and_then(Value::as_array)
                    .is_some_and(|members| {
                        members.len() == 1
                            && members[0].get("member_id").and_then(Value::as_str) == Some("_self")
                    })
        });
    if !owner_only {
        return;
    }
    let Some(items) = candidate.get_mut("items").and_then(Value::as_array_mut) else {
        return;
    };
    for annotation in items
        .iter_mut()
        .filter_map(|item| item.get_mut("member_annotations"))
        .filter_map(Value::as_array_mut)
        .flatten()
    {
        if annotation.get("member_id").and_then(Value::as_str) == Some("_self")
            && annotation.get("label").is_none_or(Value::is_null)
        {
            annotation["label"] = json!("you");
        }
    }
}

fn private_identifier_shaped(value: &str) -> bool {
    contains_private_household_identifier(value)
        || EvaluationMemberId::parse(value.trim().to_owned()).is_ok()
}

fn household_output_is_private_safe(
    household: &heyfood_core::HouseholdContext,
    output: &str,
) -> bool {
    !contains_private_household_identifier(output)
        && !household
            .members
            .iter()
            .any(|member| output.contains(member.member_id.as_str()))
}

/// Detect stable household identifiers and snapshot hashes in untrusted human
/// prose. This is deliberately token-based: ordinary hyphenated words remain
/// displayable, while UUIDs, `_self`, and 64-hex fingerprints are rejected.
#[must_use]
pub fn contains_private_household_identifier(value: &str) -> bool {
    value
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        })
        .filter(|token| !token.is_empty())
        .any(|token| {
            token == "_self"
                || is_uuid_shaped(token)
                || (token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
        })
}

fn is_uuid_shaped(value: &str) -> bool {
    let mut parts = value.split('-');
    [8_usize, 4, 4, 4, 12].into_iter().all(|expected| {
        parts.next().is_some_and(|part| {
            part.len() == expected && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) && parts.next().is_none()
}

fn append_member_annotation(output: &mut String, annotation: &MemberAnnotation, width: usize) {
    let label = inline_text(annotation.label.as_str());
    match annotation.disposition {
        AnnotationDisposition::Flag => {
            push_wrapped(
                output,
                "  ",
                "    ",
                &format!(
                    "{label}: {} — {}",
                    status_label(annotation.status),
                    explanation(&annotation.summary, annotation.status)
                ),
                width,
            );
        }
        AnnotationDisposition::Excluded => {
            let guidance = if annotation.reason.as_deref() == Some("uncertain") {
                "Allergen information is uncertain. Verify ingredients with the restaurant; when uncertain, avoid this item."
                    .to_owned()
            } else if let Some(allergen) = annotation
                .allergen
                .as_deref()
                .filter(|allergen| !allergen.trim().is_empty())
            {
                format!(
                    "It conflicts with the {} restriction. {}",
                    inline_text(allergen),
                    explanation(&annotation.summary, annotation.status)
                )
            } else {
                format!(
                    "{} Verify ingredients before ordering.",
                    explanation(&annotation.summary, annotation.status)
                )
            };
            push_wrapped(
                output,
                "  ",
                "    ",
                &format!(
                    "{label}: Excluded from recommendations — {}. {guidance}",
                    status_label(annotation.status)
                ),
                width,
            );
        }
    }

    for conflict in &annotation.conflicts {
        push_wrapped(
            output,
            "    ",
            "      ",
            &format!(
                "Conflict: {} — {}",
                inline_text(&conflict.ingredient),
                inline_text(&conflict.reason)
            ),
            width,
        );
    }
    if let Some(alignment) = annotation.diet_alignment {
        let fit = match alignment {
            DietAlignment::Aligned => "Aligned",
            DietAlignment::Partial => "Partly aligned",
            DietAlignment::OffDiet => "Off diet",
            DietAlignment::NotAssessed => "Not assessed",
        };
        let text = annotation.diet_alignment_reason.as_deref().map_or_else(
            || format!("Diet fit: {fit}"),
            |reason| format!("Diet fit: {fit} — {}", inline_text(reason)),
        );
        push_wrapped(output, "    ", "      ", &text, width);
    }
}

fn status_label(status: SafetyStatus) -> &'static str {
    match status {
        SafetyStatus::GenerallySafer => "Generally safer",
        SafetyStatus::Risky => "Risky",
        SafetyStatus::Avoid => "Avoid",
        SafetyStatus::UnableToEvaluate => "Unable to evaluate",
    }
}

fn explanation(value: &str, status: SafetyStatus) -> String {
    let value = inline_text(value);
    if !value.is_empty() {
        return value;
    }
    match status {
        SafetyStatus::UnableToEvaluate => {
            "The result is unknown; verify ingredients before deciding.".to_owned()
        }
        _ => "Detailed reasoning was not provided; verify ingredients before deciding.".to_owned(),
    }
}

fn inline_text(value: &str) -> String {
    terminal_safe_text(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn push_wrapped(
    output: &mut String,
    first_prefix: &str,
    continuation_prefix: &str,
    text: &str,
    width: usize,
) {
    let text = inline_text(text);
    let first_budget = width.saturating_sub(display_width(first_prefix)).max(1);
    let continuation_budget = width
        .saturating_sub(display_width(continuation_prefix))
        .max(1);
    let mut prefix = first_prefix;
    let mut budget = first_budget;
    let mut line = String::new();

    for word in text.split_whitespace() {
        for piece in word_pieces(word, budget) {
            let separator = usize::from(!line.is_empty());
            if !line.is_empty() && display_width(&line) + separator + display_width(&piece) > budget
            {
                output.push_str(prefix);
                output.push_str(&line);
                output.push('\n');
                prefix = continuation_prefix;
                budget = continuation_budget;
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&piece);
        }
    }
    output.push_str(prefix);
    output.push_str(&line);
    output.push('\n');
}

fn word_pieces(word: &str, maximum: usize) -> Vec<String> {
    if display_width(word) <= maximum {
        return vec![word.to_owned()];
    }
    let mut pieces = Vec::new();
    let mut current = String::new();
    for character in word.chars() {
        if display_width(&current) >= maximum {
            pieces.push(std::mem::take(&mut current));
        }
        current.push(character);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

fn display_width(value: &str) -> usize {
    value.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn founding_fixture() -> Value {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/household-backend/v1/fixtures/household_evaluation/founding_scenario_maya_menu.json"
        )))
        .unwrap()
    }

    fn founding_result() -> Value {
        founding_fixture()["result"].clone()
    }

    fn fifteen_item_result(member_count: usize) -> Value {
        assert!((1..=4).contains(&member_count));
        let mut result = founding_result();
        let member_ids = [
            "_self",
            "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01",
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
        ];
        let labels = ["Jordan", "Maya", "Sam", "Riley"];
        let mut members = Vec::new();
        let mut annotations = Vec::new();
        for index in 0..member_count {
            let mut member = result["household"]["members"][usize::from(index != 0)].clone();
            member["member_id"] = json!(member_ids[index]);
            member["profile_version"] = json!((index + 1) as u64);
            members.push(member);

            let mut annotation =
                result["items"][0]["member_annotations"][usize::from(index != 0)].clone();
            annotation["member_id"] = json!(member_ids[index]);
            annotation["label"] = json!(labels[index]);
            annotation["member_profile_version"] = json!((index + 1) as u64);
            annotations.push(annotation);
        }
        result["household"]["members"] = Value::Array(members);
        result["household"]["member_count"] = json!(member_count);

        let mut items = Vec::new();
        let mut bucket = Vec::new();
        for index in 0..15 {
            let name = format!("Dish {}", index + 1);
            let mut item = result["items"][0].clone();
            item["item_name"] = json!(name);
            item["matched_name"] = json!(name);
            item["member_annotations"] = Value::Array(annotations.clone());
            if member_count == 1 {
                item["status"] = json!("generally_safer");
                item["confidence"] = json!(0.95);
                item["summary"] = json!("No concerns.");
            }
            items.push(item);
            bucket.push(Value::String(name));
        }
        result["items"] = Value::Array(items);
        result["generally_safer"] = if member_count == 1 {
            Value::Array(bucket.clone())
        } else {
            json!([])
        };
        result["avoid"] = if member_count == 1 {
            json!([])
        } else {
            Value::Array(bucket)
        };
        result["risky"] = json!([]);
        result["unmatched"] = json!([]);
        result
    }

    #[test]
    fn finds_direct_and_nested_agent_result_shapes_without_tool_names() {
        let result = founding_result();
        for document in [
            result.clone(),
            json!({"structured": result.clone()}),
            json!({"structured_content": result.clone()}),
            json!({"structuredContent": result.clone()}),
            json!({"result": result.clone()}),
            json!({"result": {"structured_content": result.clone()}}),
        ] {
            assert!(household_evaluation_document(&document).is_some());
            assert!(render_household_evaluation(&document).unwrap().is_some());
        }
        let partial = json!({"result": {"items": [], "household": {}}});
        assert!(household_evaluation_document(&partial).is_some());
        assert!(render_household_evaluation(&partial).is_err());
    }

    #[test]
    fn founding_maya_scenario_renders_aggregate_and_named_member_annotations() {
        let rendered = render_household_evaluation(&founding_result())
            .unwrap()
            .unwrap();
        for expected in [
            "Household evaluation at Bistro One",
            "• Garlic Noodles",
            "Household result: Avoid — Garlic and onion are high-FODMAP.",
            "Jordan: Generally safer — No concerns.",
            "Maya: Avoid — Garlic and onion are high-FODMAP.",
            "• Steamed Jasmine Rice",
            "Household result: Generally safer — No concerns.",
            "Maya: Generally safer — Plain rice is low-FODMAP.",
        ] {
            assert!(
                rendered.contains(expected),
                "missing {expected:?}:\n{rendered}"
            );
        }
    }

    #[test]
    fn household_diet_alignment_is_advisory_below_each_safety_result() {
        let mut result = founding_result();
        let generally_safer = &mut result["items"][1]["member_annotations"][0];
        generally_safer["diet_alignment"] = json!("off_diet");
        generally_safer["diet_alignment_reason"] =
            json!("This preparation is outside the declared keto pattern.");
        let avoid = &mut result["items"][0]["member_annotations"][1];
        avoid["diet_alignment"] = json!("aligned");
        avoid["diet_alignment_reason"] =
            json!("This preparation fits the declared Mediterranean pattern.");

        let rendered = render_household_evaluation(&result).unwrap().unwrap();
        let semantic = inline_text(&rendered);
        assert!(semantic.contains("Jordan: Generally safer — No concerns."));
        assert!(semantic.contains(
            "Diet fit: Off diet — This preparation is outside the declared keto pattern."
        ));
        assert!(semantic.contains("Maya: Avoid — Garlic and onion are high-FODMAP."));
        assert!(semantic.contains(
            "Diet fit: Aligned — This preparation fits the declared Mediterranean pattern."
        ));
    }

    #[test]
    fn uncertain_exclusion_is_distinct_and_carries_verify_and_avoid_guidance() {
        let mut result = founding_result();
        let annotation = &mut result["items"][0]["member_annotations"][1];
        annotation["disposition"] = json!("excluded");
        annotation["allergen"] = json!("allium");
        annotation["reason"] = json!("uncertain");

        let rendered = render_household_evaluation(&result).unwrap().unwrap();
        let semantic = inline_text(&rendered);
        assert!(semantic.contains("Maya: Excluded from recommendations — Avoid."));
        assert!(semantic.contains("Allergen information is uncertain."));
        assert!(semantic.contains("Verify ingredients with the restaurant"));
        assert!(semantic.contains("when uncertain, avoid this item."));
        assert!(rendered.contains("Jordan: Generally safer — No concerns."));
        assert!(!rendered.contains("Jordan: Excluded"));
    }

    #[test]
    fn unable_to_evaluate_is_never_presented_as_avoid() {
        let mut result = founding_result();
        let item = &mut result["items"][0];
        item["status"] = json!("unable_to_evaluate");
        item["confidence"] = json!(0.2);
        item["summary"] = json!("The ingredient list is incomplete.");
        item["member_annotations"][1]["status"] = json!("unable_to_evaluate");
        item["member_annotations"][1]["confidence"] = json!(0.2);
        item["member_annotations"][1]["summary"] = json!("The ingredient list is incomplete.");
        result["avoid"] = json!([]);

        let rendered = render_household_evaluation(&result).unwrap().unwrap();
        assert!(rendered.contains("Household result: Unable to evaluate"));
        assert!(rendered.contains("Maya: Unable to evaluate"));
        assert!(!rendered.contains("Household result: Avoid"));
        assert!(!rendered.contains("Maya: Avoid"));
    }

    #[test]
    fn missing_labels_and_unknown_protocol_enums_return_only_the_safe_error() {
        let member_id = "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01";
        let context_hash = "54aa3228a67d4e262d383d0cfba6be4f4c0c94f21f5d095f3127d00928586bcb";
        for mutation in ["missing_label", "unknown_status", "unknown_disposition"] {
            let mut result = founding_result();
            match mutation {
                "missing_label" => {
                    result["items"][0]["member_annotations"][1]["label"] = Value::Null
                }
                "unknown_status" => {
                    result["items"][0]["member_annotations"][1]["status"] = json!("future_status")
                }
                "unknown_disposition" => {
                    result["items"][0]["member_annotations"][1]["disposition"] =
                        json!("future_disposition")
                }
                _ => unreachable!(),
            }
            let error = render_household_evaluation(&result).unwrap_err();
            let presented = error.to_string();
            assert_eq!(presented, UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE);
            assert!(!presented.contains(member_id));
            assert!(!presented.contains(context_hash));
            assert!(!presented.contains("future_status"));
            assert!(!presented.contains("future_disposition"));
        }
    }

    #[test]
    fn partial_household_evaluation_is_still_a_fail_closed_candidate() {
        let mut result = founding_result();
        result.as_object_mut().unwrap().remove("restaurant_name");
        assert!(household_evaluation_document(&result).is_some());
        assert_eq!(
            render_household_evaluation(&json!({
                "text": "Unreviewed household prose.",
                "structured_content": result
            }))
            .unwrap_err()
            .to_string(),
            UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE
        );
    }

    #[test]
    fn household_only_truncation_is_still_a_fail_closed_candidate() {
        let document = json!({
            "text": "Unreviewed household prose.",
            "structured_content": {
                "household": {
                    "member_count": 2
                }
            }
        });

        assert!(household_evaluation_document(&document).is_some());
        assert_eq!(
            render_household_evaluation(&document)
                .unwrap_err()
                .to_string(),
            UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE
        );
    }

    #[test]
    fn identifier_shaped_member_labels_fail_closed() {
        let mut result = founding_result();
        let member_id = result["household"]["members"][1]["member_id"].clone();
        for item in result["items"].as_array_mut().unwrap() {
            item["member_annotations"][1]["label"] = member_id.clone();
        }
        assert_eq!(
            render_household_evaluation(&result)
                .unwrap_err()
                .to_string(),
            UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE
        );
    }

    #[test]
    fn stable_member_ids_in_model_prose_fail_closed() {
        let mut result = founding_result();
        let member_id = result["household"]["members"][1]["member_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let summary = format!("This prose names {member_id}.");
        result["items"][0]["summary"] = json!(summary);
        result["items"][0]["member_annotations"][1]["summary"] = json!(summary);

        assert_eq!(
            render_household_evaluation(&result)
                .unwrap_err()
                .to_string(),
            UNPRESENTABLE_HOUSEHOLD_EVALUATION_MESSAGE
        );
    }

    #[test]
    fn presentation_never_contains_identifiers_hashes_producer_metadata_or_json_keys() {
        let rendered = render_household_evaluation(&founding_result())
            .unwrap()
            .unwrap();
        for forbidden in [
            "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01",
            "54aa3228a67d4e262d383d0cfba6be4f4c0c94f21f5d095f3127d00928586bcb",
            "stub-model-1",
            "dietary-rules-1",
            "evaluate_menu",
            "member_id",
            "context_hash",
            "member_annotations",
            "{\"",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "leaked {forbidden:?}:\n{rendered}"
            );
        }
    }

    #[test]
    fn valid_owner_only_result_with_null_labels_preserves_the_preexisting_presentation() {
        let mut result = founding_result();
        result["items"][0]["status"] = json!("generally_safer");
        result["items"][0]["confidence"] = json!(0.95);
        result["items"][0]["summary"] = json!("No concerns.");
        for item in result["items"].as_array_mut().unwrap() {
            item["member_annotations"] = Value::Array(vec![item["member_annotations"][0].clone()]);
            item["member_annotations"][0]["label"] = Value::Null;
        }
        result["generally_safer"] = json!(["Garlic Noodles", "Steamed Jasmine Rice"]);
        result["avoid"] = json!([]);
        result["household"]["members"] =
            Value::Array(vec![result["household"]["members"][0].clone()]);
        result["household"]["member_count"] = json!(1);

        assert_eq!(render_household_evaluation(&result).unwrap(), None);
    }

    #[test]
    fn selected_single_member_result_renders_named_attribution() {
        let mut result = founding_result();
        let member_id = result["household"]["members"][1]["member_id"].clone();
        result["household"]["effective_scope"] = member_id;
        result["household"]["members"] =
            Value::Array(vec![result["household"]["members"][1].clone()]);
        result["household"]["member_count"] = json!(1);
        for item in result["items"].as_array_mut().unwrap() {
            item["member_annotations"] = Value::Array(vec![item["member_annotations"][1].clone()]);
            item["status"] = item["member_annotations"][0]["status"].clone();
            item["confidence"] = item["member_annotations"][0]["confidence"].clone();
            item["summary"] = item["member_annotations"][0]["summary"].clone();
            item["conflicts"] = item["member_annotations"][0]["conflicts"].clone();
        }

        let rendered = render_household_evaluation(&result).unwrap().unwrap();
        assert!(rendered.contains("Maya: Avoid"));
        assert!(rendered.contains("Maya: Generally safer"));
        assert!(!rendered.contains("3f1c9c2e"));
    }

    #[test]
    fn semantic_output_wraps_at_40_80_and_120_columns() {
        for width in [40_usize, 80, 120] {
            let rendered = render_household_evaluation_at_width(&founding_result(), width)
                .unwrap()
                .unwrap();
            assert!(
                rendered.lines().all(|line| display_width(line) <= width),
                "width {width}:\n{rendered}"
            );
            assert!(rendered.contains("Maya:"), "width {width}:\n{rendered}");
            assert!(
                rendered.contains("Garlic Noodles"),
                "width {width}:\n{rendered}"
            );
        }
    }

    #[test]
    fn one_two_and_four_member_fifteen_item_projection_has_bounded_client_latency() {
        const ITERATIONS: usize = 128;
        const BUDGET: Duration = Duration::from_secs(2);

        for member_count in [1_usize, 2, 4] {
            let document = fifteen_item_result(member_count);
            let started = Instant::now();
            for _ in 0..ITERATIONS {
                let rendered = render_household_evaluation(&document).unwrap();
                assert_eq!(rendered.is_some(), member_count > 1);
            }
            let elapsed = started.elapsed();
            eprintln!(
                "household_client_projection members={member_count} items=15 iterations={ITERATIONS} elapsed_us={}",
                elapsed.as_micros()
            );
            assert!(
                elapsed < BUDGET,
                "{member_count}-member client projection exceeded {BUDGET:?}: {elapsed:?}"
            );
        }
    }
}
