//! Canonical dietary onboarding selections and profile construction.

use std::{collections::BTreeMap, fmt, sync::LazyLock};

use serde::Deserialize;
use serde_json::{Value, json};

const CATALOG_JSON: &str = include_str!("../../../assets/dietary/dietary_options.v2.json");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct OnboardingOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub enum_key: Option<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    deprecated: bool,
}

#[derive(Debug, Deserialize)]
struct CatalogDocument {
    version: u8,
    sections: CatalogSections,
}

#[derive(Debug, Deserialize)]
struct CatalogSections {
    health_conditions: CatalogSection,
    diet_style: CatalogSection,
    allergies: CatalogSection,
    activity_level: CatalogSection,
    cuisines: CatalogSection,
}

#[derive(Debug, Default, Deserialize)]
struct CatalogSection {
    #[serde(default)]
    tier1: Vec<OnboardingOption>,
    #[serde(default)]
    tier2: Vec<OnboardingOption>,
    #[serde(default)]
    options: Vec<OnboardingOption>,
}

impl CatalogSection {
    fn canonical_options(&self) -> Vec<OnboardingOption> {
        self.tier1
            .iter()
            .chain(&self.tier2)
            .chain(&self.options)
            .filter(|option| option.id != "__none__" && !option.deprecated)
            .cloned()
            .collect()
    }
}

#[derive(Debug)]
struct OnboardingCatalog {
    diets: Vec<OnboardingOption>,
    allergies: Vec<OnboardingOption>,
    conditions: Vec<OnboardingOption>,
    activities: Vec<OnboardingOption>,
    cuisines: Vec<OnboardingOption>,
}

static CATALOG: LazyLock<OnboardingCatalog> = LazyLock::new(|| {
    let document: CatalogDocument =
        serde_json::from_str(CATALOG_JSON).expect("embedded dietary catalog must be valid JSON");
    assert_eq!(
        document.version, 2,
        "embedded dietary catalog version changed"
    );
    OnboardingCatalog {
        diets: document.sections.diet_style.canonical_options(),
        allergies: document.sections.allergies.canonical_options(),
        conditions: document.sections.health_conditions.canonical_options(),
        activities: document.sections.activity_level.canonical_options(),
        cuisines: document.sections.cuisines.canonical_options(),
    }
});

#[must_use]
pub fn diet_options() -> &'static [OnboardingOption] {
    &CATALOG.diets
}

#[must_use]
pub fn allergy_options() -> &'static [OnboardingOption] {
    &CATALOG.allergies
}

#[must_use]
pub fn condition_options() -> &'static [OnboardingOption] {
    &CATALOG.conditions
}

#[must_use]
pub fn activity_options() -> &'static [OnboardingOption] {
    &CATALOG.activities
}

#[must_use]
pub fn cuisine_options() -> &'static [OnboardingOption] {
    &CATALOG.cuisines
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct OnboardingProfileInput {
    pub diet_style_ids: Vec<String>,
    pub custom_diet_styles: Vec<String>,
    pub allergy_ids: Vec<String>,
    pub custom_restrictions: Vec<String>,
    pub health_condition_ids: Vec<String>,
    pub custom_health_conditions: Vec<String>,
    pub avoid_ingredients: Vec<String>,
    pub activity_level: Option<String>,
    pub cuisine_preferences: Vec<String>,
    pub custom_cuisines: Vec<String>,
    pub severity_level: Option<u8>,
    pub notes: Option<String>,
}

impl fmt::Debug for OnboardingProfileInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OnboardingProfileInput")
            .field("diet_style_count", &self.diet_style_ids.len())
            .field("custom_diet_style_count", &self.custom_diet_styles.len())
            .field("allergy_count", &self.allergy_ids.len())
            .field("custom_restriction_count", &self.custom_restrictions.len())
            .field("health_condition_count", &self.health_condition_ids.len())
            .field(
                "custom_health_condition_count",
                &self.custom_health_conditions.len(),
            )
            .field("avoid_ingredient_count", &self.avoid_ingredients.len())
            .field("has_activity_level", &self.activity_level.is_some())
            .field("cuisine_count", &self.cuisine_preferences.len())
            .field("custom_cuisine_count", &self.custom_cuisines.len())
            .field("has_severity", &self.severity_level.is_some())
            .field("has_notes", &self.notes.is_some())
            .finish()
    }
}

impl OnboardingProfileInput {
    pub fn profile_data(&self) -> Result<Value, String> {
        validate_ids("diet style", &self.diet_style_ids, diet_options())?;
        validate_ids("allergy", &self.allergy_ids, allergy_options())?;
        validate_ids(
            "health condition",
            &self.health_condition_ids,
            condition_options(),
        )?;
        validate_ids("cuisine", &self.cuisine_preferences, cuisine_options())?;
        validate_custom("custom diet style", &self.custom_diet_styles, 10, 40)?;
        validate_custom("custom restriction", &self.custom_restrictions, 10, 60)?;
        validate_custom(
            "custom health condition",
            &self.custom_health_conditions,
            10,
            60,
        )?;
        validate_custom("custom cuisine", &self.custom_cuisines, 10, 40)?;
        if let Some(activity) = self.activity_level.as_deref() {
            validate_ids("activity level", &[activity.to_owned()], activity_options())?;
        }
        if self.avoid_ingredients.len() > 20 {
            return Err("at most 20 avoided ingredients may be saved".into());
        }
        for ingredient in &self.avoid_ingredients {
            validate_user_text("avoided ingredient", ingredient, 40)?;
        }
        if let Some(notes) = &self.notes {
            validate_user_text("notes", notes, 280)?;
        }
        if self.health_condition_ids.is_empty() && self.severity_level.is_some() {
            return Err("condition severity requires at least one health condition".into());
        }
        if self
            .severity_level
            .is_some_and(|severity| !(1..=5).contains(&severity))
        {
            return Err("condition severity must be between 1 and 5".into());
        }

        let mut preferences = Vec::new();
        let mut restrictions = Vec::new();
        let mut medical_constraints = Vec::new();
        for diet_id in &self.diet_style_ids {
            let option = find_option(diet_options(), diet_id);
            if let Some(restriction) = diet_restriction(diet_id) {
                push_unique(&mut restrictions, restriction);
            } else if let Some(preference) = diet_preference(option, diet_id) {
                push_unique(&mut preferences, preference);
            }
        }
        for allergy_id in &self.allergy_ids {
            if let Some(restriction) = find_option(allergy_options(), allergy_id)
                .and_then(|option| option.enum_key.as_deref())
            {
                push_unique(&mut restrictions, restriction);
            }
        }
        for condition_id in &self.health_condition_ids {
            if condition_id == "celiac" {
                push_unique(&mut restrictions, "glutenFree");
            }
            if let Some(option) = find_option(condition_options(), condition_id) {
                for constraint in &option.constraints {
                    push_unique(&mut medical_constraints, constraint);
                }
            }
        }
        for diet_id in &self.diet_style_ids {
            for constraint in diet_constraints(diet_id) {
                push_unique(&mut medical_constraints, constraint);
            }
        }

        let preference_strictness = preferences
            .iter()
            .map(|value| {
                let strictness = if matches!(value.as_str(), "keto" | "low_fodmap") {
                    "strict"
                } else {
                    "moderate"
                };
                (value.clone(), Value::String(strictness.into()))
            })
            .collect::<serde_json::Map<_, _>>();
        let restriction_handling = restrictions
            .iter()
            .map(|value| {
                (
                    value.clone(),
                    Value::String(default_restriction_handling(value).into()),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let severity = if self.health_condition_ids.is_empty() {
            None
        } else {
            Some(self.severity_level.unwrap_or(3))
        };
        let condition_severity_levels = severity.map_or_else(BTreeMap::new, |severity| {
            self.health_condition_ids
                .iter()
                .map(|condition| (condition.clone(), severity))
                .collect()
        });
        let activity = self.activity_level.as_deref();
        let custom_diet_styles = merge_custom_labels(
            &self.custom_diet_styles,
            labels_without_enum(&self.diet_style_ids, diet_options()),
        );
        let custom_restrictions = merge_custom_labels(
            &self.custom_restrictions,
            labels_without_enum(&self.allergy_ids, allergy_options()),
        );

        Ok(json!({
            "preferences": preferences,
            "preference_strictness": preference_strictness,
            "restrictions": restrictions,
            "restriction_handling": restriction_handling,
            "avoid_ingredients": self.avoid_ingredients,
            "notes": self.notes,
            "medical_condition_id": self.health_condition_ids.first(),
            "medical_constraints": medical_constraints,
            "severity_level": severity,
            "activity_level": activity,
            "cuisine_preferences": self.cuisine_preferences,
            "health_condition_ids": self.health_condition_ids,
            "custom_health_conditions": self.custom_health_conditions,
            "custom_diet_styles": custom_diet_styles,
            "custom_restrictions": custom_restrictions,
            "custom_cuisines": self.custom_cuisines,
            "selection_provenance_version": 1,
            "diet_style_ids": self.diet_style_ids,
            "allergy_ids": self.allergy_ids,
            "additional_restriction_ids": [],
            "additional_medical_constraints": [],
            "condition_severity_levels": condition_severity_levels,
        }))
    }
}

fn validate_ids(
    label: &str,
    values: &[String],
    options: &[OnboardingOption],
) -> Result<(), String> {
    if values.len() > options.len() {
        return Err(format!("too many {label} selections"));
    }
    for value in values {
        if !options.iter().any(|option| option.id == value.as_str()) {
            return Err(format!("unknown {label} selection"));
        }
    }
    Ok(())
}

fn validate_user_text(label: &str, value: &str, maximum: usize) -> Result<(), String> {
    if value.trim().is_empty()
        || value.chars().count() > maximum
        || value.chars().any(char::is_control)
    {
        return Err(format!("{label} is invalid"));
    }
    Ok(())
}

fn validate_custom(
    label: &str,
    values: &[String],
    maximum: usize,
    max_length: usize,
) -> Result<(), String> {
    if values.len() > maximum {
        return Err(format!("at most {maximum} {label} values may be saved"));
    }
    for value in values {
        validate_user_text(label, value, max_length)?;
    }
    Ok(())
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|current| current == value) {
        values.push(value.to_owned());
    }
}

fn find_option<'a>(options: &'a [OnboardingOption], id: &str) -> Option<&'a OnboardingOption> {
    options.iter().find(|option| option.id == id)
}

fn diet_restriction(diet: &str) -> Option<&'static str> {
    match diet {
        "gluten_free" => Some("glutenFree"),
        "dairy_free" => Some("dairyFree"),
        "halal" => Some("halal"),
        "kosher" => Some("kosher"),
        _ => None,
    }
}

fn diet_preference<'a>(option: Option<&'a OnboardingOption>, diet: &'a str) -> Option<&'a str> {
    let candidate = option
        .and_then(|value| value.enum_key.as_deref())
        .unwrap_or(diet);
    matches!(
        candidate,
        "keto"
            | "vegan"
            | "vegetarian"
            | "paleo"
            | "mediterranean"
            | "lowCarb"
            | "whole30"
            | "pescatarian"
            | "low_fodmap"
            | "high_protein"
            | "none"
    )
    .then_some(candidate)
}

fn diet_constraints(diet: &str) -> &'static [&'static str] {
    match diet {
        "low_fodmap" => &["high_fodmap"],
        "low_sodium" | "dash" => &["high_sodium"],
        "low_fat" => &["high_fat"],
        _ => &[],
    }
}

fn labels_without_enum(values: &[String], options: &[OnboardingOption]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| find_option(options, value))
        .filter(|option| option.enum_key.is_none())
        .map(|option| option.label.clone())
        .collect()
}

fn merge_custom_labels(explicit: &[String], derived: Vec<String>) -> Vec<String> {
    let mut values = explicit.to_vec();
    for label in derived {
        if !values.iter().any(|value| value == &label) {
            values.push(label);
        }
    }
    values
}

fn default_restriction_handling(restriction: &str) -> &'static str {
    match restriction {
        "nutFree" | "peanutFree" | "treeNutFree" | "shellfishFree" | "fishFree" | "eggFree"
        | "sesameFree" => "strictAvoid",
        "lactoseIntolerant" => "doseDependent",
        "halal" | "kosher" => "verificationRequired",
        _ => "ingredientsOnly",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_profile_derives_restrictions_constraints_and_provenance() {
        let input = OnboardingProfileInput {
            diet_style_ids: vec!["low_fodmap".into(), "low_sodium".into()],
            allergy_ids: vec!["peanuts".into()],
            health_condition_ids: vec!["celiac".into(), "ibs".into()],
            avoid_ingredients: vec!["onion".into()],
            activity_level: Some("moderate".into()),
            cuisine_preferences: vec!["thai".into()],
            severity_level: Some(4),
            notes: None,
            ..OnboardingProfileInput::default()
        };

        let profile = input.profile_data().unwrap();
        assert_eq!(
            profile["diet_style_ids"],
            json!(["low_fodmap", "low_sodium"])
        );
        assert_eq!(profile["restrictions"], json!(["peanutFree", "glutenFree"]));
        assert_eq!(
            profile["medical_constraints"],
            json!(["gluten", "high_fodmap", "carbonation", "high_sodium"])
        );
        assert_eq!(profile["preference_strictness"]["low_fodmap"], "strict");
        assert_eq!(profile["restriction_handling"]["peanutFree"], "strictAvoid");
        assert_eq!(
            profile["custom_diet_styles"],
            json!(["Low-FODMAP", "Low-sodium"])
        );
        assert_eq!(profile["condition_severity_levels"]["ibs"], 4);
        assert_eq!(profile["selection_provenance_version"], 1);
    }

    #[test]
    fn severity_without_a_condition_is_rejected() {
        let input = OnboardingProfileInput {
            severity_level: Some(4),
            ..OnboardingProfileInput::default()
        };
        assert!(input.profile_data().is_err());
    }

    #[test]
    fn canonical_defaults_match_restriction_and_activity_semantics() {
        let input = OnboardingProfileInput {
            allergy_ids: vec!["lactose".into(), "sesame".into()],
            activity_level: Some("prefer_not_to_say".into()),
            ..OnboardingProfileInput::default()
        };
        let profile = input.profile_data().unwrap();
        assert_eq!(
            profile["restrictions"],
            json!(["lactoseIntolerant", "sesameFree"])
        );
        assert_eq!(
            profile["restriction_handling"]["lactoseIntolerant"],
            "doseDependent"
        );
        assert_eq!(profile["restriction_handling"]["sesameFree"], "strictAvoid");
        assert_eq!(profile["activity_level"], "prefer_not_to_say");
    }

    #[test]
    fn debug_never_exposes_dietary_values() {
        let input = OnboardingProfileInput {
            avoid_ingredients: vec!["private ingredient".into()],
            notes: Some("private note".into()),
            ..OnboardingProfileInput::default()
        };
        let debug = format!("{input:?}");
        assert!(!debug.contains("private ingredient"));
        assert!(!debug.contains("private note"));
    }

    #[test]
    fn embedded_catalog_keeps_full_v2_option_inventory() {
        assert_eq!(condition_options().len(), 31);
        assert_eq!(diet_options().len(), 24);
        assert_eq!(allergy_options().len(), 28);
        assert_eq!(activity_options().len(), 5);
        assert_eq!(cuisine_options().len(), 28);
    }
}
