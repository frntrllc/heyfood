//! Shared human presentation for structured household-menu agent results.
//!
//! Both the one-shot CLI and interactive TUI use this formatter so a successful
//! `household_menu` result cannot be collapsed to the model's short prose
//! summary on one surface while remaining available only through `--json`.

use std::fmt::Write as _;

use heyfood_core::terminal_safe_text;
use serde_json::Value;

/// Render the structured household menu carried by an agent result.
///
/// Returns `None` for every other result type. The caller remains responsible
/// for rendering the agent's prose summary and any choices.
#[must_use]
pub fn render_household_menu(document: &Value) -> Option<String> {
    let structured = document
        .get("structured")
        .or_else(|| (document.get("type").is_some()).then_some(document))?;
    if structured.get("type").and_then(Value::as_str) != Some("household_menu") {
        return None;
    }
    if structured.get("presentation").and_then(Value::as_str) != Some("full_menu") {
        return None;
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
            append_item(&mut output, item);
        }
    }

    Some(output)
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
    if let Some(lineage) = lineage.filter(|value| !value.trim().is_empty()) {
        let _ = writeln!(output, "Source lineage: {}", inline_text(lineage));
    }
}

fn append_item(output: &mut String, item: &Value) {
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
    append_member_safety(output, item, safety.as_deref());
    append_allergen_detail(output, item);
}

fn append_member_safety(output: &mut String, item: &Value, composite: Option<&str>) {
    let Some(safety) = item.get("safety").and_then(Value::as_object) else {
        append_missing_reason_warning(output, composite);
        return;
    };

    let mut entries: Vec<_> = safety.iter().collect();
    entries.sort_by(|(left_id, left), (right_id, right)| {
        let left_label = left.get("label").and_then(Value::as_str).unwrap_or(left_id);
        let right_label = right
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or(right_id);
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
        let label = member
            .get("label")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(inline_text)
            .unwrap_or_else(|| {
                if member_id.trim().is_empty() {
                    "Household member".into()
                } else {
                    inline_text(member_id)
                }
            });
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
            .filter(|value| !value.trim().is_empty());
        let source = allergen
            .get("source")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());

        let _ = write!(output, "  Allergen flag: {}", inline_text(name));
        if confidence.is_some() || source.is_some() {
            let details = [confidence, source]
                .into_iter()
                .flatten()
                .map(inline_text)
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
        other => inline_text(other).replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
            "Source lineage: hunter_toast_sites",
            "Bread",
            "• Big Country  $9.00  [avoid]",
            "  Country sourdough.",
            "  Why for Jane (avoid): Contains wheat flour (Celiac)",
            "    Flags: Contains gluten, Shared equipment",
            "    Conflicts: Not suitable for Jane",
            "  Allergen flag: wheat (high, owner_added)",
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
    fn ignores_non_household_results() {
        assert!(render_household_menu(&json!({"type": "general_response"})).is_none());
        assert!(
            render_household_menu(&json!({"structured": {"type": "action_confirmation"}}))
                .is_none()
        );
        assert!(
            render_household_menu(&json!({
                "structured": {
                    "type": "household_menu",
                    "sections": []
                }
            }))
            .is_none()
        );
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
}
