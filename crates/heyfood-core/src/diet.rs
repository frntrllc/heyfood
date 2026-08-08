//! Contract-derived diet catalog domain values.

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;

use crate::{
    DietCatalogEntryWire, DietCatalogResponseWire, DietContraindicatedConditionWire,
    DietDetailResponseWire, DietDetailStatusWire, DietEvidenceLevelWire, DietPrincipleSectionsWire,
};

pub const DIET_CONTRACT_VERSION: u16 = 1;
pub const MAX_DIET_CATALOG_ENTRIES: usize = 30;
pub const MAX_DIET_SECTION_PARAGRAPHS: usize = 64;
pub const MAX_DIET_CITATIONS: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DietCapability {
    Unavailable,
    V1,
    UnsupportedVersion(String),
}

impl DietCapability {
    #[must_use]
    pub fn from_advertised(value: Option<&str>) -> Self {
        match value {
            None => Self::Unavailable,
            Some("v1") => Self::V1,
            Some(value) => Self::UnsupportedVersion(value.to_owned()),
        }
    }

    #[must_use]
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::V1)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DietEvidenceLevel {
    Strong,
    Moderate,
    Limited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DietDetailStatus {
    Covered,
    DietNotCovered,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DietCatalogEntry {
    pub id: String,
    pub label: String,
    pub tier: u8,
    pub evidence_level: Option<DietEvidenceLevel>,
    pub covered: bool,
    pub summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DietCatalog {
    pub diets: Vec<DietCatalogEntry>,
    pub count: usize,
    pub corpus_available: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DietPrincipleSections {
    pub principles: Vec<String>,
    pub foods_emphasized: Vec<String>,
    pub foods_limited: Vec<String>,
    pub evidence: Vec<String>,
    pub safety: Vec<String>,
    pub nutrient_adequacy: Vec<String>,
    pub restaurant_application: Vec<String>,
    pub interactions: Vec<String>,
    pub misconceptions: Vec<String>,
}

impl DietPrincipleSections {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.principles.is_empty()
            && self.foods_emphasized.is_empty()
            && self.foods_limited.is_empty()
            && self.evidence.is_empty()
            && self.safety.is_empty()
            && self.nutrient_adequacy.is_empty()
            && self.restaurant_application.is_empty()
            && self.interactions.is_empty()
            && self.misconceptions.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DietContraindicatedCondition {
    pub condition_id: String,
    pub condition_label: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DietDetail {
    pub id: String,
    pub label: String,
    pub tier: u8,
    pub evidence_level: Option<DietEvidenceLevel>,
    pub covered: bool,
    pub detail_status: DietDetailStatus,
    pub summary: String,
    pub sections: DietPrincipleSections,
    pub citations: Vec<String>,
    pub contraindicated_conditions: Vec<DietContraindicatedCondition>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DietContractError(&'static str);

impl fmt::Display for DietContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for DietContractError {}

impl TryFrom<DietCatalogResponseWire> for DietCatalog {
    type Error = DietContractError;

    fn try_from(wire: DietCatalogResponseWire) -> Result<Self, Self::Error> {
        if wire.diets.len() > MAX_DIET_CATALOG_ENTRIES || wire.count != wire.diets.len() {
            return Err(DietContractError("diet catalog count is inconsistent"));
        }
        let mut ids = BTreeSet::new();
        let diets = wire
            .diets
            .into_iter()
            .map(DietCatalogEntry::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        if diets.iter().any(|diet| !ids.insert(diet.id.as_str())) {
            return Err(DietContractError("diet catalog contains duplicate ids"));
        }
        if !wire.corpus_available && diets.iter().any(|diet| diet.covered) {
            return Err(DietContractError(
                "diet catalog claims coverage without an available corpus",
            ));
        }
        Ok(Self {
            count: diets.len(),
            diets,
            corpus_available: wire.corpus_available,
        })
    }
}

impl TryFrom<DietCatalogEntryWire> for DietCatalogEntry {
    type Error = DietContractError;

    fn try_from(wire: DietCatalogEntryWire) -> Result<Self, Self::Error> {
        validate_text(&wire.id, 64, false, "diet id is invalid")?;
        validate_text(&wire.label, 120, false, "diet label is invalid")?;
        validate_text(&wire.summary, 280, true, "diet summary is invalid")?;
        if !matches!(wire.tier, 1 | 2)
            || wire.covered == wire.summary.is_empty()
            || wire.covered != wire.evidence_level.is_some()
        {
            return Err(DietContractError("diet catalog entry is inconsistent"));
        }
        Ok(Self {
            id: wire.id,
            label: wire.label,
            tier: wire.tier,
            evidence_level: wire.evidence_level.map(Into::into),
            covered: wire.covered,
            summary: wire.summary,
        })
    }
}

impl TryFrom<DietDetailResponseWire> for DietDetail {
    type Error = DietContractError;

    fn try_from(wire: DietDetailResponseWire) -> Result<Self, Self::Error> {
        validate_text(&wire.id, 64, false, "diet id is invalid")?;
        validate_text(&wire.label, 120, false, "diet label is invalid")?;
        validate_text(&wire.summary, 280, true, "diet summary is invalid")?;
        if !matches!(wire.tier, 1 | 2) || wire.citations.len() > MAX_DIET_CITATIONS {
            return Err(DietContractError("diet detail is outside its bounds"));
        }
        let sections = DietPrincipleSections::try_from(wire.sections)?;
        let detail_status: DietDetailStatus = wire.detail_status.into();
        if wire.covered != matches!(detail_status, DietDetailStatus::Covered)
            || wire.covered != wire.evidence_level.is_some()
            || (!wire.covered
                && (!wire.summary.is_empty()
                    || wire.evidence_level.is_some()
                    || !sections.is_empty()
                    || !wire.citations.is_empty()))
        {
            return Err(DietContractError("diet coverage fields are contradictory"));
        }
        validate_list(&wire.citations, MAX_DIET_CITATIONS, 2_048)?;
        let contraindicated_conditions = wire
            .contraindicated_conditions
            .into_iter()
            .map(DietContraindicatedCondition::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            id: wire.id,
            label: wire.label,
            tier: wire.tier,
            evidence_level: wire.evidence_level.map(Into::into),
            covered: wire.covered,
            detail_status,
            summary: wire.summary,
            sections,
            citations: wire.citations,
            contraindicated_conditions,
        })
    }
}

impl TryFrom<DietPrincipleSectionsWire> for DietPrincipleSections {
    type Error = DietContractError;

    fn try_from(wire: DietPrincipleSectionsWire) -> Result<Self, Self::Error> {
        for values in [
            &wire.principles,
            &wire.foods_emphasized,
            &wire.foods_limited,
            &wire.evidence,
            &wire.safety,
            &wire.nutrient_adequacy,
            &wire.restaurant_application,
            &wire.interactions,
            &wire.misconceptions,
        ] {
            validate_list(values, MAX_DIET_SECTION_PARAGRAPHS, 16_384)?;
        }
        Ok(Self {
            principles: wire.principles,
            foods_emphasized: wire.foods_emphasized,
            foods_limited: wire.foods_limited,
            evidence: wire.evidence,
            safety: wire.safety,
            nutrient_adequacy: wire.nutrient_adequacy,
            restaurant_application: wire.restaurant_application,
            interactions: wire.interactions,
            misconceptions: wire.misconceptions,
        })
    }
}

impl TryFrom<DietContraindicatedConditionWire> for DietContraindicatedCondition {
    type Error = DietContractError;

    fn try_from(wire: DietContraindicatedConditionWire) -> Result<Self, Self::Error> {
        validate_text(
            &wire.condition_id,
            64,
            false,
            "diet condition id is invalid",
        )?;
        validate_text(
            &wire.condition_label,
            120,
            false,
            "diet condition label is invalid",
        )?;
        validate_text(&wire.reason, 400, false, "diet condition reason is invalid")?;
        Ok(Self {
            condition_id: wire.condition_id,
            condition_label: wire.condition_label,
            reason: wire.reason,
        })
    }
}

impl From<DietEvidenceLevelWire> for DietEvidenceLevel {
    fn from(value: DietEvidenceLevelWire) -> Self {
        match value {
            DietEvidenceLevelWire::Strong => Self::Strong,
            DietEvidenceLevelWire::Moderate => Self::Moderate,
            DietEvidenceLevelWire::Limited => Self::Limited,
        }
    }
}

impl From<DietDetailStatusWire> for DietDetailStatus {
    fn from(value: DietDetailStatusWire) -> Self {
        match value {
            DietDetailStatusWire::Covered => Self::Covered,
            DietDetailStatusWire::DietNotCovered => Self::DietNotCovered,
        }
    }
}

fn validate_list(
    values: &[String],
    maximum_items: usize,
    maximum_characters: usize,
) -> Result<(), DietContractError> {
    if values.len() > maximum_items {
        return Err(DietContractError("diet text list has too many entries"));
    }
    for value in values {
        validate_text(value, maximum_characters, false, "diet text is invalid")?;
    }
    Ok(())
}

fn validate_text(
    value: &str,
    maximum_characters: usize,
    allow_empty: bool,
    message: &'static str,
) -> Result<(), DietContractError> {
    if (!allow_empty && value.is_empty())
        || value.chars().count() > maximum_characters
        || value.chars().any(char::is_control)
    {
        Err(DietContractError(message))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn fixture(name: &str) -> Value {
        let source = match name {
            "catalog" => include_str!(
                "../../../fixtures/contracts/diet-backend/v1/fixtures/diet/catalog.json"
            ),
            "covered" => include_str!(
                "../../../fixtures/contracts/diet-backend/v1/fixtures/diet/detail_covered.json"
            ),
            "not-covered" => include_str!(
                "../../../fixtures/contracts/diet-backend/v1/fixtures/diet/detail_not_covered.json"
            ),
            _ => unreachable!(),
        };
        serde_json::from_str(source).unwrap()
    }

    #[test]
    fn frozen_catalog_converts_to_bounded_domain() {
        let wire: DietCatalogResponseWire =
            serde_json::from_value(fixture("catalog")["response"].clone()).unwrap();
        let catalog = DietCatalog::try_from(wire).unwrap();
        assert_eq!(catalog.count, 22);
        assert!(catalog.diets.iter().all(|diet| diet.covered));
    }

    #[test]
    fn covered_and_uncovered_cards_preserve_contract_semantics() {
        let covered: DietDetailResponseWire =
            serde_json::from_value(fixture("covered")["response"].clone()).unwrap();
        let uncovered: DietDetailResponseWire =
            serde_json::from_value(fixture("not-covered")["response"].clone()).unwrap();
        let covered = DietDetail::try_from(covered).unwrap();
        let uncovered = DietDetail::try_from(uncovered).unwrap();
        assert_eq!(covered.detail_status, DietDetailStatus::Covered);
        assert!(!covered.sections.safety.is_empty());
        assert_eq!(uncovered.detail_status, DietDetailStatus::DietNotCovered);
        assert!(uncovered.sections.is_empty());
        assert!(!uncovered.contraindicated_conditions.is_empty());
    }

    #[test]
    fn wire_objects_tolerate_additive_optional_fields() {
        let mut response = fixture("catalog")["response"].clone();
        response["future_catalog_field"] = Value::Bool(true);
        response["diets"][0]["future_entry_field"] = Value::String("ignored".into());
        let wire: DietCatalogResponseWire = serde_json::from_value(response).unwrap();
        assert_eq!(DietCatalog::try_from(wire).unwrap().count, 22);
    }
}
