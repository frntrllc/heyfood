//! Provider-neutral household evaluation contract imported from hellofood.
//!
//! The frozen source and founding fixture live under
//! `fixtures/contracts/household-backend/v1`. These DTOs intentionally contain
//! no provider SDK types or secrets. They validate the safety-critical enum,
//! attribution, snapshot, and aggregation semantics before presentation code
//! can consume the result.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use uuid::Uuid;

pub const HOUSEHOLD_EVALUATION_CONTRACT_VERSION: u64 = 1;
pub const HOUSEHOLD_EVALUATION_SOURCE_COMMIT: &str = "a1c4455fbc8a52e4073d6921dfd9a73b7f95537e";
pub const HOUSEHOLD_EVALUATION_SOURCE_TREE: &str = "9739da513d001d7db3363c454a7f66ab286b0d6c";
pub const HOUSEHOLD_EVALUATION_CONTRACT_SHA256: &str =
    "295e57714894845d55ee6cc95684235db76c64454c0a620cf3c6118d7a84ccdf";
pub const HOUSEHOLD_EVALUATION_FIXTURE_SHA256: &str =
    "f1056049ce6d4e3d99fc8ec4006a1c91bbb3583caae874559697f69f1ae588a0";
pub const HOUSEHOLD_EVALUATION_AGGREGATE_SHA256: &str =
    "2a469fbe8d14de09a8c41e4b984ef90eef59c62f1575e3987ff28823ca41ad83";

/// Distinguishes malformed JSON/wire values from cross-field contract errors.
#[derive(Debug)]
pub enum HouseholdEvaluationError {
    Json(serde_json::Error),
    Semantic(String),
}

impl fmt::Display for HouseholdEvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "invalid household evaluation JSON: {error}"),
            Self::Semantic(message) => {
                write!(
                    formatter,
                    "invalid household evaluation contract: {message}"
                )
            }
        }
    }
}

impl Error for HouseholdEvaluationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Semantic(_) => None,
        }
    }
}

impl From<serde_json::Error> for HouseholdEvaluationError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct EvaluationMemberId(String);

impl EvaluationMemberId {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value == "_self" {
            return Ok(Self(value));
        }
        let parsed = Uuid::parse_str(&value)
            .map_err(|_| "evaluation member ID must be _self or a canonical UUID")?;
        if parsed.hyphenated().to_string() != value {
            return Err("evaluation member ID must be _self or a canonical lowercase UUID");
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_self(&self) -> bool {
        self.0 == "_self"
    }
}

impl fmt::Debug for EvaluationMemberId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_self() {
            formatter.write_str("EvaluationMemberId(_self)")
        } else {
            formatter.write_str("EvaluationMemberId([REDACTED])")
        }
    }
}

impl Serialize for EvaluationMemberId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for EvaluationMemberId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct HumanLabel(String);

impl HumanLabel {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err("member annotation label must not be blank");
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HumanLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HumanLabel([REDACTED])")
    }
}

impl Serialize for HumanLabel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HumanLabel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct EvaluationConfidence(f64);

impl EvaluationConfidence {
    pub fn new(value: f64) -> Result<Self, &'static str> {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err("evaluation confidence must be finite and between 0 and 1")
        }
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for EvaluationConfidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(f64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct EvaluationContextHash(String);

impl EvaluationContextHash {
    pub fn parse(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("household evaluation context hash must be 64 lowercase hex characters");
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for EvaluationContextHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EvaluationContextHash([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for EvaluationContextHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct EvaluationContextHashVersion(u64);

impl EvaluationContextHashVersion {
    pub fn new(value: u64) -> Result<Self, &'static str> {
        (value > 0)
            .then_some(Self(value))
            .ok_or("household evaluation context hash version must be positive")
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for EvaluationContextHashVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct EvaluationProfileVersion(u64);

impl EvaluationProfileVersion {
    pub fn new(value: u64) -> Result<Self, &'static str> {
        (value > 0)
            .then_some(Self(value))
            .ok_or("household evaluation profile version must be positive")
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for EvaluationProfileVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyStatus {
    GenerallySafer,
    Risky,
    Avoid,
    UnableToEvaluate,
}

impl SafetyStatus {
    #[must_use]
    pub const fn severity(self) -> u8 {
        match self {
            Self::GenerallySafer => 0,
            Self::Risky => 1,
            Self::UnableToEvaluate => 2,
            Self::Avoid => 3,
        }
    }

    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::GenerallySafer => "generally_safer",
            Self::Risky => "risky",
            Self::Avoid => "avoid",
            Self::UnableToEvaluate => "unable_to_evaluate",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationDisposition {
    Flag,
    Excluded,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationProfileSource {
    Persisted,
    Request,
    Ephemeral,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationConsentState {
    Granted,
    Revoked,
    NotApplicable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvaluationScope {
    Self_,
    Everyone,
    Member(EvaluationMemberId),
}

impl EvaluationScope {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Self_ => "_self",
            Self::Everyone => "everyone",
            Self::Member(member_id) => member_id.as_str(),
        }
    }
}

impl Serialize for EvaluationScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EvaluationScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "_self" => Ok(Self::Self_),
            "everyone" => Ok(Self::Everyone),
            _ => EvaluationMemberId::parse(value)
                .map(Self::Member)
                .map_err(serde::de::Error::custom),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationConflict {
    pub ingredient: String,
    pub reason: String,
    pub category: String,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

impl EvaluationConflict {
    fn validate(&self) -> Result<(), String> {
        for (field, value) in [
            ("ingredient", self.ingredient.as_str()),
            ("reason", self.reason.as_str()),
            ("category", self.category.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("member conflict {field} must not be blank"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HouseholdMemberRef {
    pub member_id: EvaluationMemberId,
    pub profile_version: Option<EvaluationProfileVersion>,
    pub profile_source: EvaluationProfileSource,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HouseholdContext {
    pub effective_scope: EvaluationScope,
    pub members: Vec<HouseholdMemberRef>,
    pub member_count: usize,
    pub consent_state: EvaluationConsentState,
    pub context_hash: EvaluationContextHash,
    pub context_hash_version: EvaluationContextHashVersion,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Deserialize)]
struct HouseholdContextWire {
    effective_scope: EvaluationScope,
    #[serde(default)]
    members: Vec<HouseholdMemberRef>,
    member_count: usize,
    consent_state: EvaluationConsentState,
    context_hash: EvaluationContextHash,
    context_hash_version: EvaluationContextHashVersion,
    #[serde(default, flatten)]
    extra: Map<String, Value>,
}

impl TryFrom<HouseholdContextWire> for HouseholdContext {
    type Error = String;

    fn try_from(value: HouseholdContextWire) -> Result<Self, Self::Error> {
        let context = Self {
            effective_scope: value.effective_scope,
            members: value.members,
            member_count: value.member_count,
            consent_state: value.consent_state,
            context_hash: value.context_hash,
            context_hash_version: value.context_hash_version,
            extra: value.extra,
        };
        context.validate()?;
        Ok(context)
    }
}

impl<'de> Deserialize<'de> for HouseholdContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        HouseholdContextWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl HouseholdContext {
    fn validate(&self) -> Result<(), String> {
        if self.members.is_empty() {
            return Err("household evaluation scope must contain at least one member".into());
        }
        if self.member_count != self.members.len() {
            return Err("household member_count does not match members length".into());
        }
        let mut member_ids = HashSet::with_capacity(self.members.len());
        for member in &self.members {
            if !member_ids.insert(member.member_id.as_str()) {
                return Err("household evaluation members contain a duplicate member ID".into());
            }
        }
        match &self.effective_scope {
            EvaluationScope::Self_ => {
                if self.members.len() != 1 || !self.members[0].member_id.is_self() {
                    return Err("_self scope must resolve to the owner alone".into());
                }
            }
            EvaluationScope::Everyone => {
                if !self.members.iter().any(|member| member.member_id.is_self()) {
                    return Err("everyone scope must include the account owner".into());
                }
            }
            EvaluationScope::Member(member_id) => {
                if self.members.len() != 1 || self.members[0].member_id != *member_id {
                    return Err("member scope must resolve to exactly that member".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MemberAnnotation {
    pub member_id: EvaluationMemberId,
    pub label: HumanLabel,
    pub disposition: AnnotationDisposition,
    pub status: SafetyStatus,
    pub confidence: EvaluationConfidence,
    pub summary: String,
    pub conflicts: Vec<EvaluationConflict>,
    pub allergen: Option<String>,
    pub reason: Option<String>,
    pub model_version: String,
    pub rules_version: String,
    pub context_hash: EvaluationContextHash,
    pub context_hash_version: EvaluationContextHashVersion,
    pub member_profile_version: Option<EvaluationProfileVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diet_alignment: Option<DietAlignment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diet_alignment_reason: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Deserialize)]
struct MemberAnnotationWire {
    member_id: EvaluationMemberId,
    label: HumanLabel,
    disposition: AnnotationDisposition,
    status: SafetyStatus,
    confidence: EvaluationConfidence,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    conflicts: Vec<EvaluationConflict>,
    allergen: Option<String>,
    reason: Option<String>,
    model_version: String,
    rules_version: String,
    context_hash: EvaluationContextHash,
    context_hash_version: EvaluationContextHashVersion,
    member_profile_version: Option<EvaluationProfileVersion>,
    #[serde(default)]
    diet_alignment: Option<DietAlignment>,
    #[serde(default)]
    diet_alignment_reason: Option<String>,
    #[serde(default, flatten)]
    extra: Map<String, Value>,
}

impl TryFrom<MemberAnnotationWire> for MemberAnnotation {
    type Error = String;

    fn try_from(value: MemberAnnotationWire) -> Result<Self, Self::Error> {
        let annotation = Self {
            member_id: value.member_id,
            label: value.label,
            disposition: value.disposition,
            status: value.status,
            confidence: value.confidence,
            summary: value.summary,
            conflicts: value.conflicts,
            allergen: value.allergen,
            reason: value.reason,
            model_version: value.model_version,
            rules_version: value.rules_version,
            context_hash: value.context_hash,
            context_hash_version: value.context_hash_version,
            member_profile_version: value.member_profile_version,
            diet_alignment: value.diet_alignment,
            diet_alignment_reason: value.diet_alignment_reason,
            extra: value.extra,
        };
        annotation.validate()?;
        Ok(annotation)
    }
}

impl<'de> Deserialize<'de> for MemberAnnotation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        MemberAnnotationWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl MemberAnnotation {
    fn validate(&self) -> Result<(), String> {
        if self.model_version.trim().is_empty() {
            return Err("member annotation model_version must not be blank".into());
        }
        if self.rules_version.trim().is_empty() {
            return Err("member annotation rules_version must not be blank".into());
        }
        for conflict in &self.conflicts {
            conflict.validate()?;
        }
        match (self.diet_alignment, self.diet_alignment_reason.as_deref()) {
            (None, None) | (Some(DietAlignment::NotAssessed), None) => {}
            (Some(_), Some(reason))
                if !reason.trim().is_empty() && reason.chars().count() <= 300 => {}
            _ => {
                return Err(
                    "diet alignment and its bounded explanation must be present together".into(),
                );
            }
        }
        match self.disposition {
            AnnotationDisposition::Flag => {
                if self.allergen.is_some() || self.reason.is_some() {
                    return Err("flag annotations must not carry exclusion fields".into());
                }
            }
            AnnotationDisposition::Excluded => {
                if self.status != SafetyStatus::Avoid {
                    return Err("excluded annotations must fail closed with avoid status".into());
                }
                if self
                    .allergen
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
                {
                    return Err("excluded annotations require a nonblank allergen".into());
                }
                if self
                    .reason
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
                {
                    return Err("excluded annotations require a nonblank reason".into());
                }
            }
        }
        Ok(())
    }
}

/// Advisory fit against a declared diet. This value never changes safety.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DietAlignment {
    Aligned,
    Partial,
    OffDiet,
    NotAssessed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluateMenuItem {
    pub item_name: String,
    pub matched_name: Option<String>,
    pub status: SafetyStatus,
    pub confidence: EvaluationConfidence,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub conflicts: Vec<EvaluationConflict>,
    #[serde(default)]
    pub allergen_flags: Vec<String>,
    #[serde(default)]
    pub member_annotations: Vec<MemberAnnotation>,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

impl EvaluateMenuItem {
    fn validate_base(&self) -> Result<(), String> {
        if self.item_name.trim().is_empty() {
            return Err("evaluated menu item name must not be blank".into());
        }
        if self
            .matched_name
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err("evaluated menu matched name must not be blank".into());
        }
        for conflict in &self.conflicts {
            conflict.validate()?;
        }
        if self
            .allergen_flags
            .iter()
            .any(|value| value.trim().is_empty())
        {
            return Err("evaluated menu allergen flags must not be blank".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EvaluateMenuResponse {
    pub restaurant_id: String,
    pub restaurant_name: String,
    pub items: Vec<EvaluateMenuItem>,
    pub generally_safer: Vec<String>,
    pub risky: Vec<String>,
    pub avoid: Vec<String>,
    pub unmatched: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub household: Option<HouseholdContext>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Deserialize)]
struct EvaluateMenuResponseWire {
    restaurant_id: String,
    restaurant_name: String,
    #[serde(default)]
    items: Vec<EvaluateMenuItem>,
    #[serde(default)]
    generally_safer: Vec<String>,
    #[serde(default)]
    risky: Vec<String>,
    #[serde(default)]
    avoid: Vec<String>,
    #[serde(default)]
    unmatched: Vec<String>,
    #[serde(default)]
    household: Option<HouseholdContext>,
    #[serde(default, flatten)]
    extra: Map<String, Value>,
}

impl TryFrom<EvaluateMenuResponseWire> for EvaluateMenuResponse {
    type Error = String;

    fn try_from(value: EvaluateMenuResponseWire) -> Result<Self, Self::Error> {
        let response = Self {
            restaurant_id: value.restaurant_id,
            restaurant_name: value.restaurant_name,
            items: value.items,
            generally_safer: value.generally_safer,
            risky: value.risky,
            avoid: value.avoid,
            unmatched: value.unmatched,
            household: value.household,
            extra: value.extra,
        };
        response.validate()?;
        Ok(response)
    }
}

impl<'de> Deserialize<'de> for EvaluateMenuResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        EvaluateMenuResponseWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl EvaluateMenuResponse {
    pub fn parse_slice(bytes: &[u8]) -> Result<Self, HouseholdEvaluationError> {
        let wire = serde_json::from_slice::<EvaluateMenuResponseWire>(bytes)?;
        wire.try_into().map_err(HouseholdEvaluationError::Semantic)
    }

    pub fn parse_value(value: Value) -> Result<Self, HouseholdEvaluationError> {
        let wire = serde_json::from_value::<EvaluateMenuResponseWire>(value)?;
        wire.try_into().map_err(HouseholdEvaluationError::Semantic)
    }

    fn validate(&self) -> Result<(), String> {
        if self.restaurant_id.trim().is_empty() || self.restaurant_name.trim().is_empty() {
            return Err("evaluated menu restaurant identity must not be blank".into());
        }
        for item in &self.items {
            item.validate_base()?;
        }
        match &self.household {
            None => {
                if self
                    .items
                    .iter()
                    .any(|item| !item.member_annotations.is_empty())
                {
                    return Err("member annotations require household snapshot identity".into());
                }
            }
            Some(household) => {
                for item in &self.items {
                    self.validate_household_item(household, item)?;
                }
            }
        }
        self.validate_buckets()
    }

    fn validate_household_item(
        &self,
        household: &HouseholdContext,
        item: &EvaluateMenuItem,
    ) -> Result<(), String> {
        if item.member_annotations.is_empty() {
            if item.matched_name.is_none() && item.status == SafetyStatus::UnableToEvaluate {
                return Ok(());
            }
            return Err("matched household menu items require one annotation per member".into());
        }
        if item.member_annotations.len() != household.members.len() {
            return Err("member annotation cardinality does not match resolved household".into());
        }
        for (annotation, member) in item.member_annotations.iter().zip(&household.members) {
            if annotation.member_id != member.member_id {
                return Err("member annotations must follow resolved household order".into());
            }
            if annotation.member_profile_version != member.profile_version {
                return Err("member annotation profile version does not match snapshot".into());
            }
            if annotation.context_hash != household.context_hash
                || annotation.context_hash_version != household.context_hash_version
            {
                return Err("member annotation context identity does not match snapshot".into());
            }
        }
        let mut decisive = &item.member_annotations[0];
        for candidate in &item.member_annotations[1..] {
            if candidate.status.severity() > decisive.status.severity()
                || (candidate.status == decisive.status
                    && candidate.confidence.get() < decisive.confidence.get())
            {
                decisive = candidate;
            }
        }
        if item.status != decisive.status
            || item.confidence != decisive.confidence
            || item.summary != decisive.summary
            || item.conflicts != decisive.conflicts
        {
            return Err("menu item headline does not match worst-member aggregation".into());
        }
        Ok(())
    }

    fn validate_buckets(&self) -> Result<(), String> {
        let generally_safer = self
            .items
            .iter()
            .filter(|item| item.status == SafetyStatus::GenerallySafer)
            .map(|item| item.item_name.as_str())
            .collect::<Vec<_>>();
        let risky = self
            .items
            .iter()
            .filter(|item| item.status == SafetyStatus::Risky)
            .map(|item| item.item_name.as_str())
            .collect::<Vec<_>>();
        let avoid = self
            .items
            .iter()
            .filter(|item| item.status == SafetyStatus::Avoid)
            .map(|item| item.item_name.as_str())
            .collect::<Vec<_>>();
        let unmatched = self
            .items
            .iter()
            .filter(|item| {
                item.status == SafetyStatus::UnableToEvaluate && item.matched_name.is_none()
            })
            .map(|item| item.item_name.as_str())
            .collect::<Vec<_>>();
        if generally_safer != strings_as_str(&self.generally_safer)
            || risky != strings_as_str(&self.risky)
            || avoid != strings_as_str(&self.avoid)
            || unmatched != strings_as_str(&self.unmatched)
        {
            return Err("evaluate_menu status buckets do not match item results".into());
        }
        Ok(())
    }
}

fn strings_as_str(values: &[String]) -> Vec<&str> {
    values.iter().map(String::as_str).collect()
}

/// Attribution-only projection accepted from either log-meal preview or
/// confirmed response. All non-attribution response fields round-trip in
/// `extra`; the meal still resolves to exactly one household member.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MealAttribution {
    pub member_id: EvaluationMemberId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub household: Option<HouseholdContext>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Deserialize)]
struct MealAttributionWire {
    member_id: EvaluationMemberId,
    #[serde(default)]
    household: Option<HouseholdContext>,
    #[serde(default, flatten)]
    extra: Map<String, Value>,
}

impl TryFrom<MealAttributionWire> for MealAttribution {
    type Error = String;

    fn try_from(value: MealAttributionWire) -> Result<Self, Self::Error> {
        let result = Self {
            member_id: value.member_id,
            household: value.household,
            extra: value.extra,
        };
        result.validate()?;
        Ok(result)
    }
}

impl<'de> Deserialize<'de> for MealAttribution {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        MealAttributionWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl MealAttribution {
    pub fn parse_value(value: Value) -> Result<Self, HouseholdEvaluationError> {
        let wire = serde_json::from_value::<MealAttributionWire>(value)?;
        wire.try_into().map_err(HouseholdEvaluationError::Semantic)
    }

    fn validate(&self) -> Result<(), String> {
        let Some(household) = &self.household else {
            if self.member_id.is_self() {
                return Ok(());
            }
            return Err("non-owner meal attribution requires household snapshot identity".into());
        };
        if !household
            .members
            .iter()
            .any(|member| member.member_id == self.member_id)
        {
            return Err("meal attribution member is outside the resolved household".into());
        }
        match &household.effective_scope {
            EvaluationScope::Self_ if !self.member_id.is_self() => {
                Err("_self meal scope must attribute to the owner".into())
            }
            EvaluationScope::Everyone if !self.member_id.is_self() => {
                Err("everyone meal scope must attribute one meal to the owner".into())
            }
            EvaluationScope::Member(member_id) if *member_id != self.member_id => {
                Err("explicit member meal scope must attribute to that member".into())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    const CONTRACT: &[u8] = include_bytes!(
        "../../../fixtures/contracts/household-backend/v1/household-evaluation-contract.json"
    );
    const FIXTURE: &[u8] = include_bytes!(
        "../../../fixtures/contracts/household-backend/v1/fixtures/household_evaluation/founding_scenario_maya_menu.json"
    );
    const PROVENANCE: &[u8] =
        include_bytes!("../../../fixtures/contracts/household-backend/v1/provenance.json");

    fn founding_result() -> Value {
        serde_json::from_slice::<Value>(FIXTURE).unwrap()["result"].clone()
    }

    #[test]
    fn imported_contract_fixture_and_aggregate_match_reviewed_digests() {
        let contract_digest = format!("{:x}", Sha256::digest(CONTRACT));
        let fixture_digest = format!("{:x}", Sha256::digest(FIXTURE));
        assert_eq!(contract_digest, HOUSEHOLD_EVALUATION_CONTRACT_SHA256);
        assert_eq!(fixture_digest, HOUSEHOLD_EVALUATION_FIXTURE_SHA256);

        let canonical = format!(
            "fixtures/household_evaluation/founding_scenario_maya_menu.json\t{fixture_digest}\n\
             household-evaluation-contract.json\t{contract_digest}\n"
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(canonical.as_bytes())),
            HOUSEHOLD_EVALUATION_AGGREGATE_SHA256
        );

        let provenance: Value = serde_json::from_slice(PROVENANCE).unwrap();
        assert_eq!(
            provenance["reviewed_source_commit"],
            HOUSEHOLD_EVALUATION_SOURCE_COMMIT
        );
        assert_eq!(
            provenance["reviewed_source_tree"],
            HOUSEHOLD_EVALUATION_SOURCE_TREE
        );
        assert_eq!(
            provenance["aggregate"]["digest"],
            HOUSEHOLD_EVALUATION_AGGREGATE_SHA256
        );
        assert_eq!(
            provenance["deployment_evidence"]["status"],
            "pending_external_qualification"
        );
        assert!(provenance["deployment_evidence"]["render_deploy_id"].is_null());
        assert!(provenance["deployment_evidence"]["live_commit"].is_null());
    }

    #[test]
    fn founding_fixture_parses_and_pins_worst_member_aggregation() {
        let response = EvaluateMenuResponse::parse_value(founding_result()).unwrap();
        assert_eq!(response.household.as_ref().unwrap().member_count, 2);
        assert_eq!(response.items[0].status, SafetyStatus::Avoid);
        assert_eq!(
            response.items[0].member_annotations[1].label.as_str(),
            "Maya"
        );
        assert_eq!(
            response.items[1].status,
            SafetyStatus::GenerallySafer,
            "the less-confident owner reading wins the equal-status tie"
        );
        assert_eq!(serde_json::to_value(response).unwrap(), founding_result());
    }

    #[test]
    fn diet_alignment_parses_additively_without_mutating_safety() {
        let fixture: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/diet-backend/v1/fixtures/diet/alignment_payload.json"
        )))
        .unwrap();
        let mut result = founding_result();
        for (item_index, fixture_key) in [
            (1, "flag_on_off_diet_but_safe"),
            (0, "flag_on_aligned_but_unsafe"),
        ] {
            let source = &fixture[fixture_key];
            let annotation = &mut result["items"][item_index]["member_annotations"][0];
            annotation["diet_alignment"] = source["diet_alignment"].clone();
            annotation["diet_alignment_reason"] = source["diet_alignment_reason"].clone();
        }
        let response = EvaluateMenuResponse::parse_value(result).unwrap();
        assert_eq!(response.items[1].status, SafetyStatus::GenerallySafer);
        assert_eq!(
            response.items[1].member_annotations[0].diet_alignment,
            Some(DietAlignment::OffDiet)
        );
        assert_eq!(response.items[0].status, SafetyStatus::Avoid);
        assert_eq!(
            response.items[0].member_annotations[0].diet_alignment,
            Some(DietAlignment::Aligned)
        );
    }

    #[test]
    fn single_member_contract_and_additive_fields_round_trip_without_loss() {
        let input = serde_json::json!({
            "restaurant_id": "rest-1",
            "restaurant_name": "Bistro One",
            "items": [{
                "item_name": "Rice",
                "matched_name": "Rice",
                "status": "generally_safer",
                "confidence": 0.95,
                "summary": "No concerns.",
                "conflicts": [],
                "allergen_flags": [],
                "member_annotations": [{
                    "member_id": "_self",
                    "label": "Jordan",
                    "disposition": "flag",
                    "status": "generally_safer",
                    "confidence": 0.95,
                    "summary": "No concerns.",
                    "conflicts": [],
                    "allergen": null,
                    "reason": null,
                    "model_version": "stub-model-1",
                    "rules_version": "dietary-rules-1",
                    "context_hash": "54aa3228a67d4e262d383d0cfba6be4f4c0c94f21f5d095f3127d00928586bcb",
                    "context_hash_version": 1,
                    "member_profile_version": 1,
                    "future_annotation_field": {"kept": true}
                }],
                "future_item_field": [1, 2, 3]
            }],
            "generally_safer": ["Rice"],
            "risky": [],
            "avoid": [],
            "unmatched": [],
            "household": {
                "effective_scope": "everyone",
                "members": [{
                    "member_id": "_self",
                    "profile_version": 1,
                    "profile_source": "persisted"
                }],
                "member_count": 1,
                "consent_state": "not_applicable",
                "context_hash": "54aa3228a67d4e262d383d0cfba6be4f4c0c94f21f5d095f3127d00928586bcb",
                "context_hash_version": 1,
                "future_household_field": "kept"
            },
            "future_result_field": 7
        });
        let parsed = EvaluateMenuResponse::parse_value(input.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), input);
    }

    #[test]
    fn malformed_unknown_and_missing_human_fields_fail_closed() {
        assert!(matches!(
            EvaluateMenuResponse::parse_slice(br#"{"restaurant_id":"rest-1""#),
            Err(HouseholdEvaluationError::Json(_))
        ));

        let mut unknown_status = founding_result();
        unknown_status["items"][0]["status"] = Value::String("safe".into());
        assert!(matches!(
            EvaluateMenuResponse::parse_value(unknown_status),
            Err(HouseholdEvaluationError::Json(_))
        ));

        let mut unknown_disposition = founding_result();
        unknown_disposition["items"][0]["member_annotations"][0]["disposition"] =
            Value::String("warning".into());
        assert!(matches!(
            EvaluateMenuResponse::parse_value(unknown_disposition),
            Err(HouseholdEvaluationError::Json(_))
        ));

        let mut missing_label = founding_result();
        missing_label["items"][0]["member_annotations"][0]
            .as_object_mut()
            .unwrap()
            .remove("label");
        assert!(matches!(
            EvaluateMenuResponse::parse_value(missing_label),
            Err(HouseholdEvaluationError::Json(_))
        ));

        let mut blank_label = founding_result();
        blank_label["items"][0]["member_annotations"][0]["label"] = Value::String("  ".into());
        assert!(matches!(
            EvaluateMenuResponse::parse_value(blank_label),
            Err(HouseholdEvaluationError::Json(_))
        ));

        let mut missing_c2 = founding_result();
        missing_c2["items"][0]["member_annotations"][0]
            .as_object_mut()
            .unwrap()
            .remove("model_version");
        assert!(matches!(
            EvaluateMenuResponse::parse_value(missing_c2),
            Err(HouseholdEvaluationError::Json(_))
        ));

        let mut blank_c2 = founding_result();
        blank_c2["items"][0]["member_annotations"][0]["rules_version"] =
            Value::String(String::new());
        assert!(matches!(
            EvaluateMenuResponse::parse_value(blank_c2),
            Err(HouseholdEvaluationError::Json(_))
        ));

        let mut invalid_confidence = founding_result();
        invalid_confidence["items"][0]["member_annotations"][0]["confidence"] =
            serde_json::json!(1.01);
        assert!(matches!(
            EvaluateMenuResponse::parse_value(invalid_confidence),
            Err(HouseholdEvaluationError::Json(_))
        ));
    }

    #[test]
    fn snapshot_aggregation_and_exclusion_invariants_fail_closed() {
        let mut stale_snapshot = founding_result();
        stale_snapshot["items"][0]["member_annotations"][0]["context_hash"] = Value::String(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        );
        assert!(matches!(
            EvaluateMenuResponse::parse_value(stale_snapshot),
            Err(HouseholdEvaluationError::Semantic(_))
        ));

        let mut softened_unknown = founding_result();
        softened_unknown["items"][0]["member_annotations"][1]["status"] =
            Value::String("unable_to_evaluate".into());
        softened_unknown["items"][0]["member_annotations"][1]["confidence"] =
            serde_json::json!(0.0);
        softened_unknown["items"][0]["status"] = Value::String("risky".into());
        softened_unknown["items"][0]["confidence"] = serde_json::json!(0.0);
        softened_unknown["risky"] = serde_json::json!(["Garlic Noodles"]);
        softened_unknown["avoid"] = serde_json::json!([]);
        assert!(matches!(
            EvaluateMenuResponse::parse_value(softened_unknown),
            Err(HouseholdEvaluationError::Semantic(_))
        ));

        let mut uncertain_exclusion =
            founding_result()["items"][0]["member_annotations"][1].clone();
        uncertain_exclusion["disposition"] = Value::String("excluded".into());
        uncertain_exclusion["status"] = Value::String("risky".into());
        uncertain_exclusion["allergen"] = Value::String("onion".into());
        uncertain_exclusion["reason"] = Value::String("uncertain".into());
        assert!(serde_json::from_value::<MemberAnnotation>(uncertain_exclusion).is_err());

        let mut fail_closed_exclusion =
            founding_result()["items"][0]["member_annotations"][1].clone();
        fail_closed_exclusion["disposition"] = Value::String("excluded".into());
        fail_closed_exclusion["allergen"] = Value::String("onion".into());
        fail_closed_exclusion["reason"] = Value::String("uncertain".into());
        assert!(serde_json::from_value::<MemberAnnotation>(fail_closed_exclusion).is_ok());
    }

    #[test]
    fn meal_attribution_is_exactly_one_resolved_member() {
        let household = founding_result()["household"].clone();
        let valid = MealAttribution::parse_value(serde_json::json!({
            "meal_id": "meal-1",
            "member_id": "_self",
            "household": household,
            "confirmed": true
        }))
        .unwrap();
        assert_eq!(valid.member_id.as_str(), "_self");

        let household = founding_result()["household"].clone();
        let wrong_everyone_member = serde_json::json!({
            "meal_id": "meal-1",
            "member_id": "3f1c9c2e-2f5a-4a5b-8f1e-9d2b7c6a4e01",
            "household": household,
            "confirmed": true
        });
        assert!(matches!(
            MealAttribution::parse_value(wrong_everyone_member),
            Err(HouseholdEvaluationError::Semantic(_))
        ));
    }
}
