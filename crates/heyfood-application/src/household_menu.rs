//! Shared human presentation for structured household-menu agent results.
//!
//! Both the one-shot CLI and interactive TUI use this formatter so a successful
//! `household_menu` result cannot be collapsed to the model's short prose
//! summary on one surface while remaining available only through `--json`.

use std::fmt::Write as _;

use heyfood_core::terminal_safe_text;
use serde_json::Value;

use crate::household_evaluation::contains_private_household_identifier;

const MAX_MENU_NESTING_DEPTH: usize = 4;

/// A privacy-safe refusal used when a household menu contains protocol values
/// this client cannot map to reviewed human copy.
pub const UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE: &str = "hey.food returned household guidance this version can’t display safely. Update heyfood, or ask about a specific item.";

/// Render the structured household menu carried by an agent result.
///
/// Returns `None` for every other result type. The caller remains responsible
/// for rendering the agent's prose summary and any choices.
#[must_use]
pub fn render_household_menu(document: &Value) -> Option<String> {
    let structured = household_menu_candidate_document(document)?;
    if structured.get("type").and_then(Value::as_str) != Some("household_menu")
        || !household_menu_is_presentable(structured)
    {
        return Some(UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE.to_owned());
    }
    if structured.get("presentation").and_then(Value::as_str) != Some("full_menu") {
        return Some(private_safe_menu_output(
            structured,
            render_household_recommendations(structured),
        ));
    }

    let restaurant_name = structured
        .get("restaurant_name")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && *value != "Unknown")
        .unwrap_or("Restaurant");
    let explicitly_stale = structured.get("is_stale").and_then(Value::as_bool) == Some(true);
    let requested_max_age_hours = structured
        .get("requested_max_age_seconds")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value / 3600.0);
    let freshness_hours = structured
        .get("freshness_hours")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0);
    let verification_age_hours = structured
        .get("verification_age_hours")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0);
    let effective_age_hours = verification_age_hours.or(freshness_hours);
    let current_is_verified = structured.get("is_stale").and_then(Value::as_bool) == Some(false)
        && effective_age_hours
            .zip(requested_max_age_hours)
            .is_some_and(|(age, ceiling)| age <= ceiling);

    let heading = if current_is_verified {
        "Current menu at"
    } else {
        "Most recently captured menu at"
    };
    let mut output = format!("{heading} {}\n", inline_text(restaurant_name));
    append_provenance(
        &mut output,
        structured,
        current_is_verified,
        explicitly_stale,
    );

    let sections = structured
        .get("sections")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let section_count = sections
        .iter()
        .filter(|section| {
            section
                .get("items")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
        })
        .count();
    let item_count = sections
        .iter()
        .filter_map(|section| section.get("items").and_then(Value::as_array))
        .map(Vec::len)
        .sum::<usize>();
    let _ = writeln!(
        output,
        "{section_count} sections · {item_count} items · Page Up/Page Down to browse"
    );
    for section in sections {
        let Some(items) = section.get("items").and_then(Value::as_array) else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        let section_name = section
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Menu");
        let _ = writeln!(output, "\n{}", inline_text(section_name));
        for item in items {
            append_item(&mut output, structured, item);
        }
    }

    Some(private_safe_menu_output(structured, output))
}

/// Return the supported household-menu payload from an agent result document.
///
/// Deployed streams have used both a nested `structured` envelope and a
/// top-level structured document. Keeping this selector shared prevents the
/// renderer and TUI behavior from accepting different wire shapes.
#[must_use]
pub fn household_menu_document(document: &Value) -> Option<&Value> {
    let structured = household_menu_candidate_document(document)?;
    (structured.get("type").and_then(Value::as_str) == Some("household_menu")).then_some(structured)
}

fn household_menu_candidate_document(document: &Value) -> Option<&Value> {
    select_household_menu_candidate(document, 0)
}

fn select_household_menu_candidate(document: &Value, depth: usize) -> Option<&Value> {
    if document.get("type").and_then(Value::as_str) == Some("household_menu")
        || looks_household_menu_sensitive(document)
    {
        return Some(document);
    }
    if depth >= MAX_MENU_NESTING_DEPTH {
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
            .and_then(|value| select_household_menu_candidate(value, depth + 1))
        {
            return Some(candidate);
        }
    }
    None
}

fn looks_household_menu_sensitive(document: &Value) -> bool {
    let Some(object) = document.as_object() else {
        return false;
    };
    if object.contains_key("member_summaries") || object.contains_key("agent_picks") {
        return true;
    }
    object
        .get("sections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|section| section.get("items").and_then(Value::as_array))
        .flatten()
        .filter_map(|item| item.get("safety").and_then(Value::as_object))
        .any(|safety| !safety.is_empty() && safety.values().all(Value::is_object))
}

/// Whether an agent result carries a full household-menu presentation.
#[must_use]
pub fn is_full_household_menu(document: &Value) -> bool {
    household_menu_document(document).is_some_and(|structured| {
        structured.get("presentation").and_then(Value::as_str) == Some("full_menu")
    })
}

fn render_household_recommendations(structured: &Value) -> String {
    let restaurant_name = structured
        .get("restaurant_name")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && *value != "Unknown")
        .unwrap_or("this restaurant");
    let mut output = format!("Top picks at {}\n", inline_text(restaurant_name));
    append_recommendation_provenance(&mut output, structured);

    let mut member_ids = structured
        .get("member_summaries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|summary| summary.get("member_id").and_then(Value::as_str))
        .filter(|member_id| {
            structured
                .get("agent_picks")
                .and_then(Value::as_object)
                .and_then(|picks| picks.get(*member_id))
                .is_some()
        })
        .collect::<Vec<_>>();
    if let Some(picks) = structured.get("agent_picks").and_then(Value::as_object) {
        let mut remaining = picks
            .keys()
            .map(String::as_str)
            .filter(|member_id| !member_ids.contains(member_id))
            .collect::<Vec<_>>();
        remaining.sort_unstable();
        member_ids.extend(remaining);
    }

    let mut rendered_picks = 0_usize;
    for member_id in member_ids {
        let Some(picks) = structured
            .get("agent_picks")
            .and_then(Value::as_object)
            .and_then(|all_picks| all_picks.get(member_id))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let mut member_output = String::new();
        let mut member_pick_number = 0_usize;
        for pick in picks.iter().take(5) {
            let Some(item_id) = pick.get("item_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(item) = menu_item_by_id(structured, item_id) else {
                continue;
            };
            let Some(member_safety) = item
                .get("safety")
                .and_then(Value::as_object)
                .and_then(|safety| safety.get(member_id))
            else {
                continue;
            };
            let Some(level) = member_safety
                .get("level")
                .and_then(Value::as_str)
                .map(safety_label)
                .filter(|level| level == "generally safer")
            else {
                continue;
            };
            let Some(name) = item
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            else {
                continue;
            };
            member_pick_number += 1;
            let _ = write!(member_output, "{member_pick_number}. {}", inline_text(name));
            if let Some(price) = item
                .get("price_cents")
                .and_then(Value::as_i64)
                .filter(|value| *value >= 0)
            {
                let _ = write!(member_output, "  {}", format_price(price));
            }
            let tag = pick
                .get("tag")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("Recommended");
            let _ = writeln!(member_output, "  [{level}] · {}", inline_text(tag));

            let reason = pick
                .get("reason")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    member_safety
                        .get("reason")
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                });
            if let Some(reason) = reason {
                let _ = writeln!(member_output, "   {}", inline_text(reason));
            } else {
                member_output.push_str(
                    "   Detailed reasoning was not provided; verify ingredients before ordering.\n",
                );
            }
        }
        if member_pick_number == 0 {
            continue;
        }
        let _ = writeln!(
            output,
            "\n{}",
            recommendation_member_heading(structured, member_id)
        );
        output.push_str(member_output.trim_end());
        output.push('\n');
        rendered_picks += member_pick_number;
    }

    if rendered_picks == 0 {
        output.push_str(
            "\nI couldn't safely match the ranked picks to this evaluated menu. Ask about a specific item instead.\n",
        );
    }
    append_recommendation_conflicts(&mut output, structured);
    output.push_str(
        "\nAsk about any pick, or say `show me the full menu` for every evaluated option.",
    );
    output
}

fn menu_item_by_id<'a>(structured: &'a Value, item_id: &str) -> Option<&'a Value> {
    structured
        .get("sections")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|section| section.get("items").and_then(Value::as_array))
        .flatten()
        .find(|item| item.get("item_id").and_then(Value::as_str) == Some(item_id))
}

fn recommendation_member_heading(structured: &Value, member_id: &str) -> String {
    if member_id == "_self" {
        return "For you".into();
    }
    structured
        .get("member_summaries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|summary| summary.get("member_id").and_then(Value::as_str) == Some(member_id))
        .and_then(|summary| summary.get("label").and_then(Value::as_str))
        .filter(|label| !label.trim().is_empty())
        .map(|label| format!("For {}", inline_text(label)))
        .unwrap_or_else(|| "For a household member".into())
}

fn append_recommendation_conflicts(output: &mut String, structured: &Value) {
    let Some(conflicts) = structured.get("conflicts").and_then(Value::as_array) else {
        return;
    };
    let mut rendered = 0_usize;
    for conflict in conflicts.iter().take(3) {
        let Some(item_name) = conflict
            .get("item_name")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        if rendered == 0 {
            output.push_str("\nHousehold notes\n");
        }
        let _ = write!(output, "• {}", inline_text(item_name));
        if let Some(recommendation) = conflict
            .get("recommendation")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            let _ = write!(output, ": {}", inline_text(recommendation));
        }
        output.push('\n');
        if let Some(reasons) = conflict.get("reasons").and_then(Value::as_array) {
            let reasons = reasons
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .filter(|value| !contains_raw_member_reference(structured, value))
                .map(inline_text)
                .collect::<Vec<_>>()
                .join("; ");
            if !reasons.is_empty() {
                let _ = writeln!(output, "  {reasons}");
            }
        }
        rendered += 1;
    }
}

fn contains_raw_member_reference(structured: &Value, value: &str) -> bool {
    contains_known_member_id(structured, value)
}

fn append_recommendation_provenance(output: &mut String, structured: &Value) {
    if structured.get("is_stale").and_then(Value::as_bool) == Some(true) {
        output.push_str("Warning: this menu may be out of date.\n");
    }
    let provenance = structured.get("provenance");
    let freshness = structured
        .get("menu_freshness")
        .and_then(Value::as_str)
        .or_else(|| provenance.and_then(|value| value.get("freshness").and_then(Value::as_str)));
    if let Some(freshness) = freshness.filter(|value| !value.trim().is_empty()) {
        let _ = writeln!(output, "Freshness: {}", inline_text(freshness));
    } else {
        output.push_str("Freshness: not provided\n");
    }
    if let Some(verified_at) = structured
        .get("last_verified_at")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        let _ = writeln!(output, "Verified: {}", inline_text(verified_at));
    } else if let Some(captured_at) = structured
        .get("captured_at")
        .and_then(Value::as_str)
        .or_else(|| provenance.and_then(|value| value.get("captured_at").and_then(Value::as_str)))
        .filter(|value| !value.trim().is_empty())
    {
        let _ = writeln!(output, "Captured: {}", inline_text(captured_at));
    }
    if let Some(source) = structured
        .get("source_url")
        .and_then(Value::as_str)
        .or_else(|| {
            provenance.and_then(|value| {
                ["source_url", "url"]
                    .into_iter()
                    .find_map(|key| value.get(key).and_then(Value::as_str))
            })
        })
        .filter(|value| !value.trim().is_empty())
    {
        let _ = writeln!(output, "Source: {}", inline_text(source));
    }
    if let Some(lineage) = structured
        .get("source_lineage")
        .and_then(Value::as_str)
        .or_else(|| {
            provenance.and_then(|value| value.get("source_lineage").and_then(Value::as_str))
        })
        .filter(|value| !value.trim().is_empty())
        .and_then(menu_source_label)
    {
        let _ = writeln!(output, "Menu source: {lineage}");
    }
}

fn append_provenance(
    output: &mut String,
    structured: &Value,
    current_is_verified: bool,
    explicitly_stale: bool,
) {
    let provenance = structured.get("provenance");
    if explicitly_stale {
        output.push_str("Warning: this menu is older than the requested freshness window.\n");
    } else if !current_is_verified {
        output.push_str(
            "Warning: this menu's freshness could not be verified for the requested window.\n",
        );
    }

    let freshness = structured
        .get("menu_freshness")
        .and_then(Value::as_str)
        .or_else(|| provenance.and_then(|value| value.get("freshness").and_then(Value::as_str)));
    if let Some(freshness) = freshness.filter(|value| !value.trim().is_empty()) {
        let _ = writeln!(output, "Freshness: {}", inline_text(freshness));
    } else if let Some(hours) = structured
        .get("freshness_hours")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
    {
        let _ = writeln!(output, "Freshness: {hours:.1} hours old");
    }

    let captured_at = structured
        .get("captured_at")
        .and_then(Value::as_str)
        .or_else(|| provenance.and_then(|value| value.get("captured_at").and_then(Value::as_str)));
    if let Some(captured_at) = captured_at.filter(|value| !value.trim().is_empty()) {
        let _ = writeln!(output, "Captured: {}", inline_text(captured_at));
    }

    if let Some(verified_at) = structured
        .get("last_verified_at")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        let status = structured
            .get("verification_status")
            .and_then(Value::as_str)
            .map(|value| match value {
                "no_change" => " unchanged",
                "menu_updated" => " after update",
                _ => "",
            })
            .unwrap_or_default();
        let _ = writeln!(output, "Verified{status}: {}", inline_text(verified_at));
    }

    let source = structured
        .get("source_url")
        .and_then(Value::as_str)
        .or_else(|| {
            provenance.and_then(|value| {
                ["source_url", "url"]
                    .into_iter()
                    .find_map(|key| value.get(key).and_then(Value::as_str))
            })
        });
    if let Some(source) = source.filter(|value| !value.trim().is_empty()) {
        let _ = writeln!(output, "Source: {}", inline_text(source));
    }

    let lineage = structured
        .get("source_lineage")
        .and_then(Value::as_str)
        .or_else(|| {
            provenance.and_then(|value| value.get("source_lineage").and_then(Value::as_str))
        });
    if let Some(lineage) = lineage
        .filter(|value| !value.trim().is_empty())
        .and_then(menu_source_label)
    {
        let _ = writeln!(output, "Menu source: {lineage}");
    }
}

fn append_item(output: &mut String, structured: &Value, item: &Value) {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Unnamed item");
    let price = item
        .get("price_cents")
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)
        .map(format_price);
    let safety = item
        .get("composite_level")
        .and_then(Value::as_str)
        .map(safety_label);

    let _ = write!(output, "• {}", inline_text(name));
    if let Some(price) = price {
        let _ = write!(output, "  {price}");
    }
    if let Some(safety) = safety.as_deref() {
        let _ = write!(output, "  [{safety}]");
    }
    output.push('\n');

    if let Some(description) = item
        .get("description")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        let _ = writeln!(output, "  {}", inline_text(description));
    }
    append_member_safety(output, structured, item, safety.as_deref());
    append_allergen_detail(output, item);
}

fn append_member_safety(
    output: &mut String,
    structured: &Value,
    item: &Value,
    composite: Option<&str>,
) {
    let Some(safety) = item.get("safety").and_then(Value::as_object) else {
        append_missing_reason_warning(output, composite);
        return;
    };

    let mut entries: Vec<_> = safety.iter().collect();
    entries.sort_by(|(left_id, left), (right_id, right)| {
        let left_label =
            household_member_label(structured, left_id, left).unwrap_or("Household member");
        let right_label =
            household_member_label(structured, right_id, right).unwrap_or("Household member");
        inline_text(left_label).cmp(&inline_text(right_label))
    });

    let mut restrictive_members = 0_u32;
    for (member_id, member) in entries {
        let level = member
            .get("level")
            .and_then(Value::as_str)
            .map(safety_label)
            .unwrap_or_else(|| "unable to evaluate".into());
        let reason = member
            .get("reason")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());
        let chips = string_array(member.get("chips"));
        let conflicts = string_array(member.get("conflicts"));
        let label = household_member_label(structured, member_id, member)
            .map(inline_text)
            .unwrap_or_else(|| "Household member".into());
        let restrictive = matches!(level.as_str(), "caution" | "avoid" | "unable to evaluate");
        if restrictive {
            restrictive_members += 1;
        }
        if reason.is_none() && chips.is_empty() && conflicts.is_empty() {
            if restrictive {
                let _ = writeln!(
                    output,
                    "  Why for {label} ({level}): details were not provided; treat this guidance conservatively."
                );
            }
            continue;
        }

        let _ = write!(output, "  Why for {label} ({level})");
        if let Some(reason) = reason {
            let _ = write!(output, ": {}", inline_text(reason));
        }
        output.push('\n');
        if !chips.is_empty() {
            let _ = writeln!(output, "    Flags: {}", chips.join(", "));
        }
        if !conflicts.is_empty() {
            let _ = writeln!(output, "    Conflicts: {}", conflicts.join(", "));
        }
    }

    if restrictive_members == 0 {
        append_missing_reason_warning(output, composite);
    }
}

fn append_missing_reason_warning(output: &mut String, composite: Option<&str>) {
    if matches!(composite, Some("caution" | "avoid" | "unable to evaluate")) {
        output
            .push_str("  Dietary details were not provided; treat this guidance conservatively.\n");
    }
}

fn append_allergen_detail(output: &mut String, item: &Value) {
    let Some(allergens) = item.get("allergen_detail").and_then(Value::as_array) else {
        return;
    };
    for allergen in allergens {
        let name = ["allergen_label", "allergen", "allergen_code"]
            .into_iter()
            .find_map(|key| allergen.get(key).and_then(Value::as_str))
            .filter(|value| !value.trim().is_empty());
        let Some(name) = name else {
            continue;
        };
        let confidence = allergen
            .get("confidence")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .and_then(allergen_confidence_label);
        let source = allergen
            .get("source")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .and_then(allergen_source_label);

        let _ = write!(
            output,
            "  Allergen flag: {}",
            inline_text(name).replace('_', " ")
        );
        if confidence.is_some() || source.is_some() {
            let details = [confidence, source]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ");
            let _ = write!(output, " ({details})");
        }
        output.push('\n');
        if let Some(evidence) = allergen
            .get("evidence")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            let _ = writeln!(output, "    Evidence: {}", inline_text(evidence));
        }
    }
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(inline_text)
                .collect()
        })
        .unwrap_or_default()
}

fn inline_text(value: &str) -> String {
    terminal_safe_text(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_price(cents: i64) -> String {
    format!("${}.{:02}", cents / 100, cents % 100)
}

fn safety_label(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "safe" | "safer" | "generally_safe" | "generally_safer" => "generally safer".into(),
        "caution" | "risky" | "risk" => "caution".into(),
        "avoid" => "avoid".into(),
        "unable" | "unknown" | "unable_to_evaluate" => "unable to evaluate".into(),
        _ => "unable to evaluate".into(),
    }
}

fn household_menu_is_presentable(structured: &Value) -> bool {
    let summaries = structured
        .get("member_summaries")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if summaries.iter().any(|summary| {
        let Some(member_id) = summary.get("member_id").and_then(Value::as_str) else {
            return false;
        };
        let label = summary.get("label").and_then(Value::as_str);
        label.is_some_and(|label| !household_label_is_presentable(structured, label))
            || (member_id != "_self" && label.is_none())
    }) {
        return false;
    }

    let sections = structured
        .get("sections")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for item in sections
        .iter()
        .filter_map(|section| section.get("items").and_then(Value::as_array))
        .flatten()
    {
        if item.get("composite_level").is_some_and(|value| {
            value
                .as_str()
                .is_none_or(|status| !known_safety_status(status))
        }) {
            return false;
        }
        if let Some(safety) = item.get("safety").and_then(Value::as_object) {
            for (member_id, member) in safety {
                if member.get("level").is_some_and(|value| {
                    value
                        .as_str()
                        .is_none_or(|status| !known_safety_status(status))
                }) || member
                    .get("label")
                    .and_then(Value::as_str)
                    .is_some_and(|label| !household_label_is_presentable(structured, label))
                    || (member_id != "_self"
                        && household_member_label(structured, member_id, member).is_none())
                {
                    return false;
                }
            }
        }
    }

    structured
        .get("agent_picks")
        .and_then(Value::as_object)
        .is_none_or(|picks| {
            picks.keys().all(|member_id| {
                member_id == "_self"
                    || summaries.iter().any(|summary| {
                        summary.get("member_id").and_then(Value::as_str) == Some(member_id)
                            && summary
                                .get("label")
                                .and_then(Value::as_str)
                                .is_some_and(|label| {
                                    household_label_is_presentable(structured, label)
                                })
                    })
            })
        })
}

fn household_member_label<'a>(
    structured: &'a Value,
    member_id: &str,
    member: &'a Value,
) -> Option<&'a str> {
    if member_id == "_self" {
        return member
            .get("label")
            .and_then(Value::as_str)
            .filter(|label| household_label_is_presentable(structured, label))
            .or(Some("you"));
    }
    member
        .get("label")
        .and_then(Value::as_str)
        .filter(|label| household_label_is_presentable(structured, label))
        .or_else(|| {
            structured
                .get("member_summaries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|summary| summary.get("member_id").and_then(Value::as_str) == Some(member_id))
                .and_then(|summary| summary.get("label").and_then(Value::as_str))
                .filter(|label| household_label_is_presentable(structured, label))
        })
}

fn household_label_is_presentable(structured: &Value, label: &str) -> bool {
    !label.trim().is_empty()
        && !contains_private_household_identifier(label)
        && !contains_known_member_id(structured, label)
}

fn contains_known_member_id(structured: &Value, value: &str) -> bool {
    let summaries = structured
        .get("member_summaries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|summary| summary.get("member_id").and_then(Value::as_str));
    let picks = structured
        .get("agent_picks")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|picks| picks.keys().map(String::as_str));
    let safety = structured
        .get("sections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|section| section.get("items").and_then(Value::as_array))
        .flatten()
        .filter_map(|item| item.get("safety").and_then(Value::as_object))
        .flat_map(|safety| safety.keys().map(String::as_str));
    summaries
        .chain(picks)
        .chain(safety)
        .filter(|member_id| !member_id.trim().is_empty())
        .any(|member_id| value.contains(member_id))
}

fn private_safe_menu_output(structured: &Value, output: String) -> String {
    if contains_private_household_identifier(&output)
        || contains_known_member_id(structured, &output)
    {
        UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE.to_owned()
    } else {
        output
    }
}

fn known_safety_status(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "safe"
            | "safer"
            | "generally_safe"
            | "generally_safer"
            | "caution"
            | "risky"
            | "risk"
            | "avoid"
            | "unable"
            | "unknown"
            | "unable_to_evaluate"
    )
}

fn menu_source_label(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "hunter_toast" | "hunter_toast_sites" => Some("Restaurant ordering page"),
        "hunter_web"
        | "hunter_firecrawl"
        | "hunter_firecrawl_v2"
        | "hunter_official_site"
        | "hunter_wix"
        | "hunter_popmenu"
        | "hunter_squarespace" => Some("Restaurant website"),
        "hunter_pdf" => Some("Published menu"),
        "owner_email" | "owner_upload" | "owner_portal" | "owner_unknown" | "restaurant_owned" => {
            Some("Provided by the restaurant")
        }
        "admin_manual_entry" => Some("Reviewed menu entry"),
        // Source lineage is an internal, extensible backend enum. Unknown
        // values are intentionally omitted until the client has deliberate
        // human copy for them; never expose protocol vocabulary by default.
        _ => None,
    }
}

fn allergen_confidence_label(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "high" => Some("high confidence"),
        "medium" => Some("medium confidence"),
        "low" => Some("low confidence"),
        _ => None,
    }
}

fn allergen_source_label(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "owner_added" | "owner_confirmed" => Some("restaurant-confirmed"),
        "llm_inferred" | "ai_inferred" | "inferred" => Some("inferred from menu details"),
        // Allergen-source values are internal protocol metadata. Evidence is
        // still rendered, but an unknown enum must not become terminal copy.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn classifies_nested_and_top_level_full_menu_documents_consistently() {
        let structured = json!({
            "type": "household_menu",
            "presentation": "full_menu",
            "sections": []
        });
        let nested = json!({"structured": structured.clone()});

        assert!(is_full_household_menu(&nested));
        assert!(is_full_household_menu(&structured));
        assert!(render_household_menu(&nested).is_some());
        assert!(render_household_menu(&structured).is_some());
        assert!(!is_full_household_menu(&json!({
            "type": "household_menu",
            "presentation": "recommendations"
        })));
        assert!(!is_full_household_menu(&json!({"type": "meal"})));
    }

    #[test]
    fn missing_type_household_menu_shape_fails_closed() {
        let document = json!({
            "text": "Unreviewed household menu prose.",
            "structured": {
                "member_summaries": [{
                    "member_id": "member-a",
                    "label": "Alex"
                }],
                "sections": []
            }
        });

        assert!(household_menu_document(&document).is_none());
        assert_eq!(
            render_household_menu(&document).unwrap(),
            UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE
        );
    }

    #[test]
    fn renders_all_sections_items_prices_and_provenance_without_ansi() {
        let document = json!({
            "text": "Two options look generally safer.",
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "restaurant_name": "Abby Jane Bakeshop",
                "source_url": "https://example.test/menu",
                "source_lineage": "hunter_toast_sites",
                "menu_freshness": "Menu updated 2 hours ago",
                "captured_at": "2026-07-26T17:27:14Z",
                "freshness_hours": 2.0,
                "requested_max_age_seconds": 86400,
                "is_stale": false,
                "sections": [
                    {
                        "name": "Bread",
                        "items": [
                            {
                                "name": "Big Country",
                                "description": "Country sourdough.",
                                "price_cents": 900,
                                "composite_level": "avoid",
                                "safety": {
                                    "member-jane": {
                                        "member_id": "member-jane",
                                        "label": "Jane",
                                        "level": "avoid",
                                        "reason": "Contains wheat flour (Celiac)",
                                        "chips": ["Contains gluten", "Shared equipment"],
                                        "conflicts": ["Not suitable for Jane"]
                                    }
                                },
                                "allergen_detail": [{
                                    "allergen_code": "wheat",
                                    "confidence": "high",
                                    "source": "owner_added",
                                    "evidence": "Owner-confirmed wheat flour"
                                }]
                            },
                            {
                                "name": "Baguette",
                                "price_cents": 400,
                                "composite_level": "caution",
                                "safety": {}
                            }
                        ]
                    },
                    {
                        "name": "Coffee",
                        "items": [
                            {
                                "name": "Drip Coffee",
                                "price_cents": 300,
                                "composite_level": "generally_safer",
                                "safety": {}
                            }
                        ]
                    }
                ]
            }
        });

        let rendered = render_household_menu(&document).unwrap();
        for expected in [
            "Current menu at Abby Jane Bakeshop",
            "Freshness: Menu updated 2 hours ago",
            "Captured: 2026-07-26T17:27:14Z",
            "Source: https://example.test/menu",
            "Menu source: Restaurant ordering page",
            "Bread",
            "• Big Country  $9.00  [avoid]",
            "  Country sourdough.",
            "  Why for Jane (avoid): Contains wheat flour (Celiac)",
            "    Flags: Contains gluten, Shared equipment",
            "    Conflicts: Not suitable for Jane",
            "  Allergen flag: wheat (high confidence, restaurant-confirmed)",
            "    Evidence: Owner-confirmed wheat flour",
            "• Baguette  $4.00  [caution]",
            "  Dietary details were not provided; treat this guidance conservatively.",
            "Coffee",
            "• Drip Coffee  $3.00  [generally safer]",
        ] {
            assert!(rendered.lines().any(|line| line == expected));
        }
        assert_eq!(rendered.matches("• ").count(), 3);
        assert!(!rendered.contains('\u{1b}'));
    }

    #[test]
    fn renders_the_production_single_profile_menu_shape_without_protocol_json() {
        let document = json!({
            "conversation_id": "conversation-1",
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "restaurant_name": "Pismo's Coastal Grill",
                "freshness_hours": 1.0,
                "requested_max_age_seconds": 86400,
                "is_stale": false,
                "sections": [{
                    "name": "Tea",
                    "items": [{
                        "allergen_detail": [],
                        "composite_level": "caution",
                        "description": null,
                        "item_id": "18fbb9d6-85a1-4e04-bd44-a8348507048c",
                        "name": "12 oz Chai Latte",
                        "price_cents": 450,
                        "safety": {
                            "_self": {
                                "chips": ["carbohydrates"],
                                "conflicts": ["carbohydrates"],
                                "label": "Me",
                                "level": "caution",
                                "member_id": "_self",
                                "reason": "Verify sweetness level; likely high carbs"
                            }
                        }
                    }]
                }]
            }
        });

        let rendered = render_household_menu(&document).unwrap();
        for expected in [
            "Current menu at Pismo's Coastal Grill",
            "Tea",
            "• 12 oz Chai Latte  $4.50  [caution]",
            "  Why for Me (caution): Verify sweetness level; likely high carbs",
            "    Flags: carbohydrates",
            "    Conflicts: carbohydrates",
        ] {
            assert!(rendered.lines().any(|line| line == expected), "{rendered}");
        }
        for protocol_fragment in [
            "item_id",
            "member_id",
            "price_cents",
            "\"safety\"",
            "_self",
            "18fbb9d6-85a1-4e04-bd44-a8348507048c",
        ] {
            assert!(!rendered.contains(protocol_fragment), "{rendered}");
        }
    }

    #[test]
    fn ignores_non_household_results() {
        assert!(render_household_menu(&json!({"type": "general_response"})).is_none());
        assert!(
            render_household_menu(&json!({"structured": {"type": "action_confirmation"}}))
                .is_none()
        );
    }

    #[test]
    fn renders_ranked_single_profile_recommendations_with_a_real_next_step() {
        let document = json!({
            "structured": {
                "type": "household_menu",
                "restaurant_name": "Harbor Cafe",
                "menu_freshness": "Menu updated 2 hours ago",
                "captured_at": "2026-07-29T17:27:14Z",
                "source_url": "https://example.test/menu",
                "source_lineage": "restaurant_owned",
                "is_stale": false,
                "member_summaries": [{
                    "member_id": "_self",
                    "label": null,
                    "safe_count": 2
                }],
                "sections": [{
                    "name": "Dinner",
                    "items": [
                        {
                            "item_id": "item-1",
                            "name": "Grilled Fish",
                            "price_cents": 2400,
                            "safety": {
                                "_self": {
                                    "level": "safe",
                                    "reason": "Fits the active dietary profile."
                                }
                            }
                        },
                        {
                            "item_id": "item-2",
                            "name": "Roasted Vegetables",
                            "safety": {
                                "_self": {
                                    "level": "generally_safer",
                                    "reason": "No conflicts found."
                                }
                            }
                        }
                    ]
                }],
                "agent_picks": {
                    "_self": [
                        {
                            "item_id": "item-1",
                            "member_id": "_self",
                            "reason": "A simple preparation with no detected conflicts.",
                            "tag": "Top pick"
                        },
                        {
                            "item_id": "item-2",
                            "member_id": "_self",
                            "reason": "A generally safer side.",
                            "tag": "Side"
                        }
                    ]
                }
            }
        });

        let rendered = render_household_menu(&document).unwrap();
        for expected in [
            "Top picks at Harbor Cafe",
            "Freshness: Menu updated 2 hours ago",
            "Captured: 2026-07-29T17:27:14Z",
            "Source: https://example.test/menu",
            "Menu source: Provided by the restaurant",
            "For you",
            "1. Grilled Fish  $24.00  [generally safer] · Top pick",
            "   A simple preparation with no detected conflicts.",
            "2. Roasted Vegetables  [generally safer] · Side",
            "   A generally safer side.",
            "Ask about any pick, or say `show me the full menu` for every evaluated option.",
        ] {
            assert!(rendered.lines().any(|line| line == expected));
        }
        assert!(!rendered.contains("_self"));
        assert!(!rendered.contains("item-1"));
    }

    #[test]
    fn recommendation_rendering_uses_member_labels_and_fails_closed() {
        let document = json!({
            "structured": {
                "type": "household_menu",
                "restaurant_name": "Household Cafe",
                "is_stale": true,
                "member_summaries": [{
                    "member_id": "member-a",
                    "label": "Alex"
                }],
                "sections": [{
                    "name": "Dinner",
                    "items": [
                        {
                            "item_id": "safe-item",
                            "name": "Bean Bowl",
                            "safety": {
                                "member-a": {
                                    "level": "safe",
                                    "reason": "No conflicts found."
                                }
                            }
                        },
                        {
                            "item_id": "unsafe-item",
                            "name": "Unsafe Pick",
                            "safety": {
                                "member-a": {
                                    "level": "avoid",
                                    "reason": "Conflict found."
                                }
                            }
                        }
                    ]
                }],
                "agent_picks": {
                    "member-a": [
                        {
                            "item_id": "safe-item",
                            "member_id": "member-a",
                            "reason": "",
                            "tag": "Top pick"
                        },
                        {
                            "item_id": "unsafe-item",
                            "member_id": "member-a",
                            "reason": "Must not be rendered.",
                            "tag": "Invalid"
                        },
                        {
                            "item_id": "missing-item",
                            "member_id": "member-a",
                            "reason": "Must not be invented.",
                            "tag": "Invalid"
                        }
                    ]
                },
                "conflicts": [{
                    "item_name": "Shared Plate",
                    "reasons": [
                        "member-a: private restriction",
                        "Different household needs"
                    ],
                    "recommendation": "Order separately"
                }]
            }
        });

        let rendered = render_household_menu(&document).unwrap();
        assert!(rendered.contains("Warning: this menu may be out of date."));
        assert!(rendered.contains("Freshness: not provided"));
        assert!(rendered.contains("For Alex"));
        assert!(rendered.contains("1. Bean Bowl  [generally safer] · Top pick"));
        assert!(rendered.contains("   No conflicts found."));
        assert!(rendered.contains("Household notes"));
        assert!(rendered.contains("• Shared Plate: Order separately"));
        assert!(rendered.contains("Different household needs"));
        assert!(!rendered.contains("private restriction"));
        assert!(!rendered.contains("Unsafe Pick"));
        assert!(!rendered.contains("missing-item"));
        assert!(!rendered.contains("member-a"));
    }

    #[test]
    fn missing_recommendation_picks_are_reported_without_inventing_items() {
        let rendered = render_household_menu(&json!({
            "structured": {
                "type": "household_menu",
                "restaurant_name": "Cafe",
                "sections": [],
                "agent_picks": {}
            }
        }))
        .unwrap();

        assert!(
            rendered.contains("I couldn't safely match the ranked picks to this evaluated menu.")
        );
        assert!(!rendered.contains("1."));
    }

    #[test]
    fn stale_menu_is_never_presented_as_current() {
        let document = json!({
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "restaurant_name": "Abby Jane Bakeshop",
                "menu_freshness": "Menu updated 3 days ago",
                "captured_at": "2026-07-23T17:27:14Z",
                "freshness_hours": 72.0,
                "is_stale": true,
                "sections": []
            }
        });
        let rendered = render_household_menu(&document).unwrap();
        assert!(rendered.starts_with("Most recently captured menu at Abby Jane Bakeshop\n"));
        assert!(
            rendered.contains("Warning: this menu is older than the requested freshness window.")
        );
        assert!(rendered.contains("Freshness: Menu updated 3 days ago"));
        assert!(!rendered.contains("Current menu at"));
    }

    #[test]
    fn missing_freshness_evidence_fails_closed() {
        let document = json!({
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "restaurant_name": "Unknown Freshness Cafe",
                "menu_freshness": "Menu freshness unknown",
                "is_stale": false,
                "sections": []
            }
        });
        let rendered = render_household_menu(&document).unwrap();
        assert!(rendered.starts_with("Most recently captured menu at Unknown Freshness Cafe\n"));
        assert!(rendered.contains(
            "Warning: this menu's freshness could not be verified for the requested window."
        ));
        assert!(!rendered.contains("Current menu at"));
    }

    #[test]
    fn recent_generation_bound_no_change_verification_is_current() {
        let document = json!({
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "restaurant_name": "Abby Jane Bakeshop",
                "menu_freshness": "Menu captured 3 days ago",
                "captured_at": "2026-07-23T17:27:14Z",
                "freshness_hours": 72.0,
                "last_verified_at": "2026-07-26T17:27:14Z",
                "verification_age_hours": 1.0,
                "verification_status": "no_change",
                "requested_max_age_seconds": 86400,
                "is_stale": false,
                "sections": []
            }
        });
        let rendered = render_household_menu(&document).unwrap();
        assert!(rendered.starts_with("Current menu at Abby Jane Bakeshop\n"));
        assert!(rendered.contains("Verified unchanged: 2026-07-26T17:27:14Z"));
        assert!(rendered.contains("Captured: 2026-07-23T17:27:14Z"));
        assert!(!rendered.contains("freshness could not be verified"));
    }

    #[test]
    fn every_restrictive_household_member_gets_an_explanation_or_warning() {
        let document = json!({
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "restaurant_name": "Household Cafe",
                "freshness_hours": 1.0,
                "requested_max_age_seconds": 86400,
                "is_stale": false,
                "sections": [{
                    "name": "Dinner",
                    "items": [{
                        "name": "Bean Bowl",
                        "composite_level": "avoid",
                        "safety": {
                            "member-jane": {
                                "label": "Jane",
                                "level": "generally_safer",
                                "reason": "Fits Jane's profile.",
                                "chips": [],
                                "conflicts": []
                            },
                            "member-bob": {
                                "label": "Bob",
                                "level": "avoid",
                                "reason": "",
                                "chips": [],
                                "conflicts": []
                            }
                        }
                    }]
                }]
            }
        });
        let rendered = render_household_menu(&document).unwrap();
        assert!(rendered.contains("Why for Jane (generally safer): Fits Jane's profile."));
        assert!(rendered.contains(
            "Why for Bob (avoid): details were not provided; treat this guidance conservatively."
        ));
    }

    #[test]
    fn sanitizes_controls_and_prevents_untrusted_line_injection() {
        let document = json!({
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "restaurant_name": "Cafe\nSource: forged\u{1b}[31m",
                "source_url": "https://example.test/menu\nFreshness: forged",
                "sections": [{
                    "name": "Lunch\tFreshness: forged\u{7}",
                    "items": [{
                        "name": "Soup\n• forged\u{1b}[2J",
                        "description": "First line\nSource: forged",
                        "price_cents": 825,
                        "composite_level": "caution",
                        "safety": {
                            "member": {
                                "label": "Jane\nSource: forged",
                                "level": "caution",
                                "reason": "Watch portions\nFreshness: forged",
                                "chips": ["IBS\nSource: forged"],
                                "conflicts": []
                            }
                        }
                    }]
                }]
            }
        });
        let rendered = render_household_menu(&document).unwrap();
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\u{7}'));
        assert!(rendered.contains("$8.25"));
        assert!(!rendered.contains("\nSource: forged"));
        assert!(!rendered.contains("\nFreshness: forged"));
        assert!(!rendered.contains("\n• forged"));
    }

    #[test]
    fn missing_member_labels_fail_closed_without_rendering_identifiers() {
        let member_id = "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01";
        let context_hash = "54aa3228a67d4e262d383d0cfba6be4f4c0c94f21f5d095f3127d00928586bcb";
        let rendered = render_household_menu(&json!({
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "context_hash": context_hash,
                "sections": [{
                    "name": "Dinner",
                    "items": [{
                        "name": "Soup",
                        "composite_level": "avoid",
                        "safety": {
                            (member_id): {
                                "level": "avoid",
                                "reason": "Contains a restricted ingredient."
                            }
                        }
                    }]
                }]
            }
        }))
        .unwrap();

        assert_eq!(rendered, UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE);
        assert!(!rendered.contains(member_id));
        assert!(!rendered.contains(context_hash));
    }

    #[test]
    fn identifier_shaped_member_labels_fail_closed() {
        let member_id = "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01";
        let rendered = render_household_menu(&json!({
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "sections": [{
                    "name": "Dinner",
                    "items": [{
                        "name": "Soup",
                        "composite_level": "avoid",
                        "safety": {
                            (member_id): {
                                "label": member_id,
                                "level": "avoid",
                                "reason": "Contains a restricted ingredient."
                            }
                        }
                    }]
                }]
            }
        }))
        .unwrap();

        assert_eq!(rendered, UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE);
        assert!(!rendered.contains(member_id));
    }

    #[test]
    fn unknown_safety_enums_fail_closed_without_protocol_copy() {
        let rendered = render_household_menu(&json!({
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "sections": [{
                    "name": "Dinner",
                    "items": [{
                        "name": "Soup",
                        "composite_level": "future_status",
                        "safety": {}
                    }]
                }]
            }
        }))
        .unwrap();

        assert_eq!(rendered, UNPRESENTABLE_HOUSEHOLD_MENU_MESSAGE);
        assert!(!rendered.contains("future_status"));
        assert!(!rendered.contains("Soup"));
    }

    #[test]
    fn humanizes_every_supported_menu_source_without_exposing_protocol_values() {
        let cases = [
            ("hunter_toast", "Restaurant ordering page"),
            ("hunter_toast_sites", "Restaurant ordering page"),
            ("hunter_web", "Restaurant website"),
            ("hunter_firecrawl", "Restaurant website"),
            ("hunter_firecrawl_v2", "Restaurant website"),
            ("hunter_official_site", "Restaurant website"),
            ("hunter_wix", "Restaurant website"),
            ("hunter_popmenu", "Restaurant website"),
            ("hunter_squarespace", "Restaurant website"),
            ("hunter_pdf", "Published menu"),
            ("owner_email", "Provided by the restaurant"),
            ("owner_upload", "Provided by the restaurant"),
            ("owner_portal", "Provided by the restaurant"),
            ("owner_unknown", "Provided by the restaurant"),
            ("restaurant_owned", "Provided by the restaurant"),
            ("admin_manual_entry", "Reviewed menu entry"),
        ];

        for (lineage, label) in cases {
            for presentation in ["recommendations", "full_menu"] {
                let rendered = render_household_menu(&json!({
                    "structured": {
                        "type": "household_menu",
                        "presentation": presentation,
                        "restaurant_name": "Cafe",
                        "source_lineage": lineage,
                        "freshness_hours": 1.0,
                        "requested_max_age_seconds": 86400,
                        "is_stale": false,
                        "sections": []
                    }
                }))
                .unwrap();
                assert!(
                    rendered.contains(&format!("Menu source: {label}")),
                    "{lineage} ({presentation}): {rendered}"
                );
                assert!(!rendered.contains(lineage), "{lineage}: {rendered}");
                assert!(!rendered.contains("Source lineage:"), "{rendered}");
            }
        }
    }

    #[test]
    fn unknown_internal_sources_fail_closed_without_losing_human_evidence() {
        let rendered = render_household_menu(&json!({
            "structured": {
                "type": "household_menu",
                "presentation": "full_menu",
                "restaurant_name": "Cafe",
                "source_url": "https://example.test/menu",
                "source_lineage": "future_internal_source",
                "freshness_hours": 1.0,
                "requested_max_age_seconds": 86400,
                "is_stale": false,
                "sections": [{
                    "name": "Lunch",
                    "items": [{
                        "name": "Soup",
                        "allergen_detail": [{
                            "allergen_code": "tree_nuts",
                            "confidence": "future_confidence_enum",
                            "source": "future_internal_source",
                            "evidence": "Listed in the menu description"
                        }]
                    }]
                }]
            }
        }))
        .unwrap();

        assert!(rendered.contains("Source: https://example.test/menu"));
        assert!(rendered.contains("Allergen flag: tree nuts"));
        assert!(rendered.contains("Evidence: Listed in the menu description"));
        assert!(!rendered.contains("future_internal_source"));
        assert!(!rendered.contains("future_confidence_enum"));
        assert!(!rendered.contains("Source lineage:"));
    }
}
