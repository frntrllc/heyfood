//! Command-independent validation shared by every presentation surface.

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationError {
    Empty,
    TooLong { maximum_bytes: usize },
    SurroundingWhitespace,
    InvalidCharacter,
    InvalidFormat,
    OutOfRange,
    NotFinite,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("value must not be empty"),
            Self::TooLong { maximum_bytes } => {
                write!(formatter, "value exceeds {maximum_bytes} bytes")
            }
            Self::SurroundingWhitespace => {
                formatter.write_str("value must not contain surrounding whitespace")
            }
            Self::InvalidCharacter => formatter.write_str("value contains an invalid character"),
            Self::InvalidFormat => formatter.write_str("value has an invalid format"),
            Self::OutOfRange => formatter.write_str("value is outside the allowed range"),
            Self::NotFinite => formatter.write_str("value must be finite"),
        }
    }
}

pub fn required_text(value: &str, maximum_characters: usize) -> Result<String, ValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::Empty);
    }
    if value.chars().count() > maximum_characters {
        return Err(ValidationError::TooLong {
            maximum_bytes: maximum_characters,
        });
    }
    if value.chars().any(|value| value.is_control()) {
        return Err(ValidationError::InvalidCharacter);
    }
    Ok(value.to_owned())
}

pub fn optional_text(
    value: Option<&str>,
    maximum_characters: usize,
) -> Result<Option<String>, ValidationError> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| required_text(value, maximum_characters))
        .transpose()
}

pub fn coordinates(latitude: f64, longitude: f64) -> Result<(f64, f64), ValidationError> {
    if !latitude.is_finite() || !longitude.is_finite() {
        return Err(ValidationError::NotFinite);
    }
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return Err(ValidationError::OutOfRange);
    }
    Ok((latitude, longitude))
}

pub fn bounded_number(value: f64, minimum: f64, maximum: f64) -> Result<f64, ValidationError> {
    if !value.is_finite() {
        return Err(ValidationError::NotFinite);
    }
    if value < minimum || value > maximum {
        return Err(ValidationError::OutOfRange);
    }
    Ok(value)
}

pub fn bounded_integer(value: i64, minimum: i64, maximum: i64) -> Result<i64, ValidationError> {
    if value < minimum || value > maximum {
        return Err(ValidationError::OutOfRange);
    }
    Ok(value)
}

pub fn iso_date(value: &str) -> Result<String, ValidationError> {
    let mut fields = value.split('-');
    let year = fields
        .next()
        .and_then(|value| value.parse::<i32>().ok())
        .ok_or(ValidationError::InvalidFormat)?;
    let month = fields
        .next()
        .and_then(|value| value.parse::<u8>().ok())
        .and_then(|value| time::Month::try_from(value).ok())
        .ok_or(ValidationError::InvalidFormat)?;
    let day = fields
        .next()
        .and_then(|value| value.parse::<u8>().ok())
        .ok_or(ValidationError::InvalidFormat)?;
    if fields.next().is_some() || value.len() != 10 {
        return Err(ValidationError::InvalidFormat);
    }
    time::Date::from_calendar_date(year, month, day).map_err(|_| ValidationError::InvalidFormat)?;
    Ok(value.to_owned())
}

pub fn choice(value: &str, choices: &[&str]) -> Result<String, ValidationError> {
    let normalized = value.trim().to_lowercase();
    choices
        .iter()
        .any(|candidate| *candidate == normalized)
        .then_some(normalized)
        .ok_or(ValidationError::InvalidFormat)
}

impl std::error::Error for ValidationError {}

/// Validate a stable identifier without normalizing it. Identifiers are used
/// in cache and idempotency boundaries, so accepting an altered spelling would
/// make two layers disagree about identity.
pub fn validate_identifier(value: &str, maximum_bytes: usize) -> Result<(), ValidationError> {
    if value.is_empty() {
        return Err(ValidationError::Empty);
    }
    if value.len() > maximum_bytes {
        return Err(ValidationError::TooLong { maximum_bytes });
    }
    if value.trim() != value {
        return Err(ValidationError::SurroundingWhitespace);
    }
    if value
        .bytes()
        .any(|value| !(value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_' | b'.' | b':')))
    {
        return Err(ValidationError::InvalidCharacter);
    }
    Ok(())
}

/// Remove terminal-control characters from untrusted presentation text while
/// retaining ordinary newlines and tabs. Renderers must still escape their own
/// markup format; this prevents service content from emitting CSI/OSC bytes.
#[must_use]
pub fn terminal_safe_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            matches!(character, '\n' | '\t')
                || (!character.is_control()
                    && !matches!(*character as u32, 0x80..=0x9f | 0x2028 | 0x2029))
        })
        .collect()
}
