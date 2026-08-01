//! Standalone, dependency-minimal native-state release verifier.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

const MAX_FLOOR_BYTES: usize = 4 * 1024;
const MAX_DECLARATION_BYTES: usize = 4 * 1024;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const NATIVE_STATE_VERSION: u64 = 2;
const CAPABILITIES: &str = concat!(
    "[\"household-account-slot-v1\",",
    "\"household-lifecycle-lock-v1\",",
    "\"household-migration-guard-v1\",",
    "\"household-teardown-journal-v1\"]"
);

fn main() -> ExitCode {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    let Some(command) = arguments.next() else {
        return usage();
    };
    if command == "--version" {
        println!("heyfood-installer {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if command == "native-state-declaration" {
        let Some(version) = arguments.next() else {
            return usage();
        };
        if arguments.next().is_some() {
            return usage();
        }
        let Some(version) = version.to_str() else {
            return failure("release version is not valid UTF-8");
        };
        if !valid_release_version(version) {
            return failure("release version is invalid");
        }
        print!("{}", expected_declaration(version));
        return ExitCode::SUCCESS;
    }
    if command != "verify-native-state" {
        return usage();
    }

    let Some(version) = arguments.next() else {
        return usage();
    };
    let Some(root_digest) = arguments.next() else {
        return usage();
    };
    let Some(floor_path) = arguments.next() else {
        return usage();
    };
    let Some(declaration_path) = arguments.next() else {
        return usage();
    };
    let Some(candidate_manifest_path) = arguments.next() else {
        return usage();
    };
    if arguments.next().is_some() {
        return usage();
    }

    let Some(version) = version.to_str() else {
        return failure("release version is not valid UTF-8");
    };
    let Some(root_digest) = root_digest.to_str() else {
        return failure("native root digest is not valid UTF-8");
    };
    if !valid_release_version(version) {
        return failure("release version is invalid");
    }
    if root_digest != "-" && !valid_lower_hex_digest(root_digest) {
        return failure("native root digest is invalid");
    }
    if (root_digest == "-") != (floor_path == "-") {
        return failure("native root digest and compatibility floor must both be absent");
    }

    match verify(
        version,
        root_digest,
        Path::new(&floor_path),
        Path::new(&declaration_path),
        Path::new(&candidate_manifest_path),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => failure(message),
    }
}

fn verify(
    version: &str,
    root_digest: &str,
    floor_path: &Path,
    declaration_path: &Path,
    candidate_manifest_path: &Path,
) -> Result<(), &'static str> {
    let expected_declaration = expected_declaration(version);
    let declaration = read_bounded(declaration_path, MAX_DECLARATION_BYTES)
        .map_err(|_| "native state release declaration is unavailable")?;
    let parsed_declaration = parse_unique_json(&declaration)
        .map_err(|_| "native state release declaration is malformed")?;
    let expected_declaration_value = parse_unique_json(expected_declaration.as_bytes())
        .expect("the compiled declaration is valid JSON");
    if parsed_declaration != expected_declaration_value {
        return Err("native state release declaration is incompatible");
    }
    if declaration != expected_declaration.as_bytes() {
        return Err("native state release declaration is not canonical");
    }

    let manifest = read_bounded(candidate_manifest_path, MAX_MANIFEST_BYTES)
        .map_err(|_| "candidate agent manifest is unavailable")?;
    let manifest =
        parse_unique_json(&manifest).map_err(|_| "candidate agent manifest is malformed")?;
    let Value::Object(manifest) = manifest else {
        return Err("candidate agent manifest must be a top-level object");
    };
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(2) {
        return Err("candidate agent manifest is not schema version 2");
    }
    let Some(candidate_declaration) = manifest.get("native_state_compatibility") else {
        return Err("candidate agent manifest has no top-level native state declaration");
    };
    if candidate_declaration != &expected_declaration_value {
        return Err("candidate and release native state declarations disagree");
    }

    if floor_path.as_os_str() != "-" {
        let expected_floor = expected_floor(root_digest);
        let floor = read_bounded(floor_path, MAX_FLOOR_BYTES)
            .map_err(|_| "native state compatibility floor is unavailable")?;
        let parsed_floor = parse_unique_json(&floor)
            .map_err(|_| "native state compatibility floor is malformed")?;
        let expected_floor_value =
            parse_unique_json(expected_floor.as_bytes()).expect("the compiled floor is valid JSON");
        if parsed_floor != expected_floor_value {
            return Err("native state compatibility floor is incompatible");
        }
        if floor != expected_floor.as_bytes() {
            return Err("native state compatibility floor is not canonical");
        }
    }
    Ok(())
}

fn expected_declaration(version: &str) -> String {
    format!(
        "{{\"binary_version\":\"{version}\",\"maximum_native_state_version\":{NATIVE_STATE_VERSION},\"native_state_capabilities\":{CAPABILITIES},\"schema_version\":1}}"
    )
}

fn expected_floor(root_digest: &str) -> String {
    format!(
        "{{\"floor_revision\":1,\"minimum_compatible_native_state_version\":{NATIVE_STATE_VERSION},\"native_root_instance_digest\":\"{root_digest}\",\"required_binary_capabilities\":{CAPABILITIES},\"schema_version\":1}}"
    )
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > u64::try_from(limit).map_err(|_| ())?
    {
        return Err(());
    }
    let bytes = fs::read(path).map_err(|_| ())?;
    if bytes.is_empty() || bytes.len() > limit {
        return Err(());
    }
    Ok(bytes)
}

#[derive(Debug, PartialEq)]
struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueJsonVisitor)
    }
}

struct UniqueJsonVisitor;

impl<'de> Visitor<'de> for UniqueJsonVisitor {
    type Value = UniqueJson;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueJson)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<UniqueJson>()? {
            values.push(value.0);
        }
        Ok(UniqueJson(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = BTreeSet::new();
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!(
                    "duplicate JSON object key {key:?}"
                )));
            }
            let value = object.next_value::<UniqueJson>()?;
            values.insert(key, value.0);
        }
        Ok(UniqueJson(Value::Object(values)))
    }
}

fn parse_unique_json(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<UniqueJson>(bytes).map(|value| value.0)
}

fn valid_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_release_version(value: &str) -> bool {
    let mut parts = value.split('.');
    let valid_part = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|byte| byte.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'))
    };
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), Some(patch), None)
            if valid_part(major) && valid_part(minor) && valid_part(patch)
    )
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: heyfood-installer <native-state-declaration VERSION|verify-native-state VERSION ROOT_DIGEST_OR_DASH FLOOR_OR_DASH RELEASE_DECLARATION CANDIDATE_MANIFEST>"
    );
    ExitCode::from(64)
}

fn failure(message: &str) -> ExitCode {
    eprintln!("heyfood installer: {message}");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::{
        CAPABILITIES, expected_declaration, expected_floor, parse_unique_json,
        valid_lower_hex_digest, valid_release_version,
    };
    use serde_json::json;

    #[test]
    fn declaration_and_floor_are_exact_canonical_bytes() {
        assert_eq!(
            expected_declaration("0.6.2"),
            format!(
                "{{\"binary_version\":\"0.6.2\",\"maximum_native_state_version\":2,\"native_state_capabilities\":{CAPABILITIES},\"schema_version\":1}}"
            )
        );
        assert_eq!(
            expected_floor(&"a".repeat(64)),
            format!(
                "{{\"floor_revision\":1,\"minimum_compatible_native_state_version\":2,\"native_root_instance_digest\":\"{}\",\"required_binary_capabilities\":{CAPABILITIES},\"schema_version\":1}}",
                "a".repeat(64)
            )
        );
    }

    #[test]
    fn version_and_digest_inputs_are_closed() {
        assert!(valid_release_version("0.6.2"));
        assert!(!valid_release_version("0.06.2"));
        assert!(!valid_release_version("0.6.2-beta"));
        assert!(valid_lower_hex_digest(&"0".repeat(64)));
        assert!(!valid_lower_hex_digest(&"A".repeat(64)));
        assert!(!valid_lower_hex_digest(&"0".repeat(63)));
    }

    #[test]
    fn structural_parser_rejects_duplicate_keys_at_every_depth() {
        assert!(parse_unique_json(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse_unique_json(br#"{"outer":{"a":1,"a":2}}"#).is_err());
        assert!(parse_unique_json(br#"[{"a":1,"a":2}]"#).is_err());
    }

    #[test]
    fn structural_parser_preserves_object_identity() {
        assert_eq!(
            parse_unique_json(br#"{"native_state_compatibility":{"schema_version":1}}"#).unwrap(),
            json!({"native_state_compatibility": {"schema_version": 1}})
        );
        assert_ne!(
            parse_unique_json(
                br#"{"text":"\"native_state_compatibility\":{\"schema_version\":1}"}"#
            )
            .unwrap(),
            json!({"native_state_compatibility": {"schema_version": 1}})
        );
    }
}
