//! Small, dependency-free JSON parser used by repository policy checks.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    pub(crate) fn object(&self, context: &str) -> Result<&BTreeMap<String, Json>, String> {
        match self {
            Self::Object(value) => Ok(value),
            _ => Err(format!("{context} must be an object")),
        }
    }

    pub(crate) fn array(&self, context: &str) -> Result<&[Json], String> {
        match self {
            Self::Array(value) => Ok(value),
            _ => Err(format!("{context} must be an array")),
        }
    }

    pub(crate) fn string(&self, context: &str) -> Result<&str, String> {
        match self {
            Self::String(value) => Ok(value),
            _ => Err(format!("{context} must be a string")),
        }
    }

    pub(crate) fn usize(&self, context: &str) -> Result<usize, String> {
        match self {
            Self::Number(value) if !value.contains(['.', 'e', 'E', '-']) => value
                .parse()
                .map_err(|_| format!("{context} must be a non-negative integer")),
            _ => Err(format!("{context} must be a non-negative integer")),
        }
    }

    pub(crate) fn boolean(&self, context: &str) -> Result<bool, String> {
        match self {
            Self::Bool(value) => Ok(*value),
            _ => Err(format!("{context} must be a boolean")),
        }
    }
}

pub(crate) fn parse(input: &str) -> Result<Json, String> {
    let mut parser = Parser { input, offset: 0 };
    let value = parser.value()?;
    parser.whitespace();
    if parser.offset != input.len() {
        return Err(format!("unexpected data at byte {}", parser.offset));
    }
    Ok(value)
}

struct Parser<'a> {
    input: &'a str,
    offset: usize,
}

impl Parser<'_> {
    fn value(&mut self) -> Result<Json, String> {
        self.whitespace();
        match self.byte() {
            Some(b'n') => self.literal("null", Json::Null),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'"') => self.string().map(Json::String),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(b'-' | b'0'..=b'9') => self.number().map(Json::Number),
            Some(_) => Err(format!("unexpected token at byte {}", self.offset)),
            None => Err("unexpected end of JSON".to_owned()),
        }
    }

    fn literal(&mut self, literal: &str, value: Json) -> Result<Json, String> {
        if self.input[self.offset..].starts_with(literal) {
            self.offset += literal.len();
            Ok(value)
        } else {
            Err(format!("invalid literal at byte {}", self.offset))
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut output = String::new();
        loop {
            let byte = self
                .byte()
                .ok_or_else(|| "unterminated string".to_owned())?;
            match byte {
                b'"' => {
                    self.offset += 1;
                    return Ok(output);
                }
                b'\\' => {
                    self.offset += 1;
                    let escape = self.take_byte("unterminated escape")?;
                    match escape {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => self.unicode_escape(&mut output)?,
                        _ => return Err(format!("invalid escape at byte {}", self.offset - 1)),
                    }
                }
                0x00..=0x1f => {
                    return Err(format!("control byte in string at byte {}", self.offset));
                }
                0x20..=0x7f => {
                    output.push(char::from(byte));
                    self.offset += 1;
                }
                _ => {
                    let character = self.input[self.offset..]
                        .chars()
                        .next()
                        .ok_or_else(|| "invalid UTF-8 in string".to_owned())?;
                    output.push(character);
                    self.offset += character.len_utf8();
                }
            }
        }
    }

    fn unicode_escape(&mut self, output: &mut String) -> Result<(), String> {
        let first = self.hex_quad()?;
        let scalar = if (0xd800..=0xdbff).contains(&first) {
            if !self.input[self.offset..].starts_with("\\u") {
                return Err("high surrogate without low surrogate".to_owned());
            }
            self.offset += 2;
            let second = self.hex_quad()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err("invalid low surrogate".to_owned());
            }
            0x1_0000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
        } else if (0xdc00..=0xdfff).contains(&first) {
            return Err("unexpected low surrogate".to_owned());
        } else {
            u32::from(first)
        };
        output.push(char::from_u32(scalar).ok_or_else(|| "invalid Unicode scalar".to_owned())?);
        Ok(())
    }

    fn hex_quad(&mut self) -> Result<u16, String> {
        if self.offset + 4 > self.input.len() {
            return Err("truncated Unicode escape".to_owned());
        }
        let digits = &self.input[self.offset..self.offset + 4];
        if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("invalid Unicode escape at byte {}", self.offset));
        }
        self.offset += 4;
        u16::from_str_radix(digits, 16).map_err(|error| error.to_string())
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut values = Vec::new();
        self.whitespace();
        if self.consume(b']') {
            return Ok(Json::Array(values));
        }
        loop {
            values.push(self.value()?);
            self.whitespace();
            if self.consume(b']') {
                return Ok(Json::Array(values));
            }
            self.expect(b',')?;
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut values = BTreeMap::new();
        self.whitespace();
        if self.consume(b'}') {
            return Ok(Json::Object(values));
        }
        loop {
            self.whitespace();
            let key = self.string()?;
            self.whitespace();
            self.expect(b':')?;
            let value = self.value()?;
            if values.insert(key.clone(), value).is_some() {
                return Err(format!("duplicate object key {key:?}"));
            }
            self.whitespace();
            if self.consume(b'}') {
                return Ok(Json::Object(values));
            }
            self.expect(b',')?;
        }
    }

    fn number(&mut self) -> Result<String, String> {
        let start = self.offset;
        self.consume(b'-');
        match self.byte() {
            Some(b'0') => self.offset += 1,
            Some(b'1'..=b'9') => {
                self.offset += 1;
                while matches!(self.byte(), Some(b'0'..=b'9')) {
                    self.offset += 1;
                }
            }
            _ => return Err(format!("invalid number at byte {start}")),
        }
        if self.consume(b'.') {
            self.digits(start)?;
        }
        if matches!(self.byte(), Some(b'e' | b'E')) {
            self.offset += 1;
            if matches!(self.byte(), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            self.digits(start)?;
        }
        Ok(self.input[start..self.offset].to_owned())
    }

    fn digits(&mut self, start: usize) -> Result<(), String> {
        let digits_start = self.offset;
        while matches!(self.byte(), Some(b'0'..=b'9')) {
            self.offset += 1;
        }
        if self.offset == digits_start {
            Err(format!("invalid number at byte {start}"))
        } else {
            Ok(())
        }
    }

    fn whitespace(&mut self) {
        while matches!(self.byte(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.offset += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(format!(
                "expected {:?} at byte {}",
                char::from(expected),
                self.offset
            ))
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.byte() == Some(expected) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn take_byte(&mut self, error: &str) -> Result<u8, String> {
        let byte = self.byte().ok_or_else(|| error.to_owned())?;
        self.offset += 1;
        Ok(byte)
    }

    fn byte(&self) -> Option<u8> {
        self.input.as_bytes().get(self.offset).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::{Json, parse};

    #[test]
    fn parses_json_and_rejects_duplicate_keys() {
        assert_eq!(
            parse(r#"{"value":"caf\u00e9","ok":true,"count":2}"#).unwrap(),
            Json::Object(std::collections::BTreeMap::from([
                ("count".into(), Json::Number("2".into())),
                ("ok".into(), Json::Bool(true)),
                ("value".into(), Json::String("café".into())),
            ]))
        );
        assert!(parse(r#"{"value":1,"value":2}"#).is_err());
    }
}
