//! Frozen audio-transcription request limits and validated response semantics.

use std::fmt;

use serde::{Deserialize, Serialize};

pub const TRANSCRIPTION_SCHEMA_VERSION: u64 = 1;
pub const TRANSCRIPTION_SCHEMA_SHA256: &str =
    "b32ebbc5860c6b11e6fb6bfd29c3296449f6583ce7900d461764c91b6eab093f";
pub const TRANSCRIPTION_CHANNELS: u16 = 1;
pub const TRANSCRIPTION_SAMPLE_WIDTH_BYTES: usize = 2;
pub const TRANSCRIPTION_WAV_HEADER_BYTES: usize = 44;
pub const TRANSCRIPTION_SAMPLE_RATE_MIN_HZ: u32 = 8_000;
pub const TRANSCRIPTION_SAMPLE_RATE_MAX_HZ: u32 = 48_000;
pub const TRANSCRIPTION_PREFERRED_SAMPLE_RATE_HZ: u32 = 16_000;
pub const TRANSCRIPTION_MAX_DURATION_SECONDS: u64 = 120;
pub const TRANSCRIPTION_MAX_AUDIO_BYTES: usize = 12_500_000;
pub const TRANSCRIPTION_MAX_REQUEST_BYTES: usize = 13_107_200;
pub const TRANSCRIPTION_MAX_TRANSCRIPT_CHARACTERS: usize = 20_000;
pub const TRANSCRIPTION_MAX_RESPONSE_DURATION_SECONDS: f64 = 3_600.0;
pub const TRANSCRIPTION_MAX_LANGUAGE_CHARACTERS: usize = 35;
pub const TRANSCRIPTION_MAX_MODEL_VERSION_CHARACTERS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionPurpose {
    Onboarding,
    Ask,
    Log,
}

impl TranscriptionPurpose {
    #[must_use]
    pub const fn as_contract_value(self) -> &'static str {
        match self {
            Self::Onboarding => "onboarding",
            Self::Ask => "ask",
            Self::Log => "log",
        }
    }
}

#[derive(Clone, PartialEq, Deserialize)]
pub struct TranscriptionWire {
    pub transcript: String,
    #[serde(default, deserialize_with = "deserialize_present_duration")]
    pub duration_seconds: Option<f64>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_model_version")]
    pub model_version: Option<String>,
}

impl fmt::Debug for TranscriptionWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TranscriptionWire([REDACTED])")
    }
}

#[derive(Clone, PartialEq)]
pub struct Transcription {
    transcript: String,
    pub duration_seconds: Option<f64>,
    pub language: Option<String>,
    pub model_version: Option<String>,
}

impl Transcription {
    pub fn from_wire(wire: TranscriptionWire) -> Result<Self, TranscriptionContractError> {
        let transcript = validate_transcript(wire.transcript)?;
        let duration_seconds = wire.duration_seconds.map(validate_duration).transpose()?;
        let language = wire
            .language
            .map(|value| validate_optional_field(value, TRANSCRIPTION_MAX_LANGUAGE_CHARACTERS))
            .transpose()?;
        let model_version = wire
            .model_version
            .map(|value| validate_optional_field(value, TRANSCRIPTION_MAX_MODEL_VERSION_CHARACTERS))
            .transpose()?;
        Ok(Self {
            transcript,
            duration_seconds,
            language,
            model_version,
        })
    }

    #[must_use]
    pub fn transcript(&self) -> &str {
        &self.transcript
    }
}

impl fmt::Debug for Transcription {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Transcription")
            .field("transcript", &"[REDACTED]")
            .field("duration_seconds", &self.duration_seconds)
            .field("language", &self.language)
            .field("model_version", &self.model_version)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranscriptionContractError {
    EmptyTranscript,
    TranscriptTooLong,
    InvalidTranscriptCharacter,
    InvalidDuration,
    InvalidLanguage,
    InvalidModelVersion,
}

impl fmt::Display for TranscriptionContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyTranscript => "transcription response contains no transcript",
            Self::TranscriptTooLong => "transcription response transcript is too long",
            Self::InvalidTranscriptCharacter => {
                "transcription response transcript contains an invalid control character"
            }
            Self::InvalidDuration => "transcription response duration is invalid",
            Self::InvalidLanguage => "transcription response language is invalid",
            Self::InvalidModelVersion => "transcription response model version is invalid",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for TranscriptionContractError {}

fn validate_transcript(value: String) -> Result<String, TranscriptionContractError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(TranscriptionContractError::EmptyTranscript);
    }
    if value.chars().count() > TRANSCRIPTION_MAX_TRANSCRIPT_CHARACTERS {
        return Err(TranscriptionContractError::TranscriptTooLong);
    }
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return Err(TranscriptionContractError::InvalidTranscriptCharacter);
    }
    Ok(trimmed.to_owned())
}

fn validate_duration(value: f64) -> Result<f64, TranscriptionContractError> {
    if value.is_finite() && (0.0..=TRANSCRIPTION_MAX_RESPONSE_DURATION_SECONDS).contains(&value) {
        Ok(value)
    } else {
        Err(TranscriptionContractError::InvalidDuration)
    }
}

fn validate_optional_field(
    value: String,
    maximum_characters: usize,
) -> Result<String, TranscriptionContractError> {
    let error = if maximum_characters == TRANSCRIPTION_MAX_LANGUAGE_CHARACTERS {
        TranscriptionContractError::InvalidLanguage
    } else {
        TranscriptionContractError::InvalidModelVersion
    };
    if value.chars().count() > maximum_characters || value.chars().any(char::is_control) {
        return Err(error);
    }
    Ok(value)
}

fn deserialize_present_duration<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    f64::deserialize(deserializer).map(Some)
}

fn deserialize_present_model_version<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

#[must_use]
pub const fn transcription_sample_rate_supported(sample_rate: u32) -> bool {
    sample_rate >= TRANSCRIPTION_SAMPLE_RATE_MIN_HZ
        && sample_rate <= TRANSCRIPTION_SAMPLE_RATE_MAX_HZ
}
