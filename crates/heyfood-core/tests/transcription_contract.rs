use heyfood_core::{
    TRANSCRIPTION_CHANNELS, TRANSCRIPTION_MAX_AUDIO_BYTES, TRANSCRIPTION_MAX_DURATION_SECONDS,
    TRANSCRIPTION_MAX_LANGUAGE_CHARACTERS, TRANSCRIPTION_MAX_MODEL_VERSION_CHARACTERS,
    TRANSCRIPTION_MAX_REQUEST_BYTES, TRANSCRIPTION_MAX_RESPONSE_DURATION_SECONDS,
    TRANSCRIPTION_MAX_TRANSCRIPT_CHARACTERS, TRANSCRIPTION_PREFERRED_SAMPLE_RATE_HZ,
    TRANSCRIPTION_SAMPLE_RATE_MAX_HZ, TRANSCRIPTION_SAMPLE_RATE_MIN_HZ,
    TRANSCRIPTION_SAMPLE_WIDTH_BYTES, TRANSCRIPTION_SCHEMA_SHA256, TRANSCRIPTION_SCHEMA_VERSION,
    TRANSCRIPTION_WAV_HEADER_BYTES, Transcription, TranscriptionContractError, TranscriptionWire,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn schema_bytes() -> &'static [u8] {
    include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/contracts/voice/transcription.schema.json"
    ))
}

fn provenance_bytes() -> &'static [u8] {
    include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/contracts/voice/provenance.json"
    ))
}

#[test]
fn frozen_transcription_schema_bytes_and_runtime_limits_match() {
    assert_eq!(
        format!("{:x}", Sha256::digest(schema_bytes())),
        TRANSCRIPTION_SCHEMA_SHA256
    );
    let schema: Value = serde_json::from_slice(schema_bytes()).unwrap();
    assert_eq!(
        schema["x-heyfood-schema-version"],
        TRANSCRIPTION_SCHEMA_VERSION
    );
    assert_eq!(
        schema["x-heyfood-audio"]["channels"],
        TRANSCRIPTION_CHANNELS
    );
    assert_eq!(
        schema["x-heyfood-audio"]["sample_width_bytes"],
        TRANSCRIPTION_SAMPLE_WIDTH_BYTES
    );
    assert_eq!(
        schema["x-heyfood-audio"]["wav_header_bytes"],
        TRANSCRIPTION_WAV_HEADER_BYTES
    );
    assert_eq!(
        schema["x-heyfood-audio"]["sample_rate_min_hz"],
        TRANSCRIPTION_SAMPLE_RATE_MIN_HZ
    );
    assert_eq!(
        schema["x-heyfood-audio"]["sample_rate_max_hz"],
        TRANSCRIPTION_SAMPLE_RATE_MAX_HZ
    );
    assert_eq!(
        schema["x-heyfood-audio"]["preferred_sample_rate_hz"],
        TRANSCRIPTION_PREFERRED_SAMPLE_RATE_HZ
    );
    assert_eq!(
        schema["x-heyfood-audio"]["max_duration_seconds"],
        TRANSCRIPTION_MAX_DURATION_SECONDS
    );
    assert_eq!(
        schema["x-heyfood-audio"]["max_audio_bytes"],
        TRANSCRIPTION_MAX_AUDIO_BYTES
    );
    assert_eq!(
        schema["x-heyfood-audio"]["max_request_bytes"],
        TRANSCRIPTION_MAX_REQUEST_BYTES
    );
    assert_eq!(
        schema["x-heyfood-response-limits"]["max_transcript_chars"],
        TRANSCRIPTION_MAX_TRANSCRIPT_CHARACTERS
    );
    assert_eq!(
        schema["x-heyfood-response-limits"]["max_duration_seconds"],
        TRANSCRIPTION_MAX_RESPONSE_DURATION_SECONDS
    );
    assert_eq!(
        schema["x-heyfood-response-limits"]["max_language_chars"],
        TRANSCRIPTION_MAX_LANGUAGE_CHARACTERS
    );
    assert_eq!(
        schema["x-heyfood-response-limits"]["max_model_version_chars"],
        TRANSCRIPTION_MAX_MODEL_VERSION_CHARACTERS
    );
    let provenance: Value = serde_json::from_slice(provenance_bytes()).unwrap();
    assert_eq!(
        provenance["source_commit"],
        "73494a57468dac83b4904ce6c390e36926f5c6fe"
    );
    assert_eq!(provenance["source_tag"], "archive/python-cli-73494a57");
    assert_eq!(provenance["source_sha256"], TRANSCRIPTION_SCHEMA_SHA256);
    assert_eq!(
        provenance["imported_path"],
        "fixtures/contracts/voice/transcription.schema.json"
    );
}

#[test]
fn transcription_success_is_bounded_redacted_and_terminal_safe() {
    let transcription = Transcription::from_wire(TranscriptionWire {
        transcript: "  Log oatmeal\nand berries  ".into(),
        duration_seconds: Some(2.5),
        language: Some("en-US".into()),
        model_version: Some("hf-transcribe-1".into()),
    })
    .unwrap();
    assert_eq!(transcription.transcript(), "Log oatmeal\nand berries");
    assert!(!format!("{transcription:?}").contains("oatmeal"));
    assert_eq!(
        Transcription::from_wire(TranscriptionWire {
            transcript: "unsafe\u{1b}sequence".into(),
            duration_seconds: None,
            language: None,
            model_version: None,
        }),
        Err(TranscriptionContractError::InvalidTranscriptCharacter)
    );
    assert_eq!(
        Transcription::from_wire(TranscriptionWire {
            transcript: "x".repeat(TRANSCRIPTION_MAX_TRANSCRIPT_CHARACTERS + 1),
            duration_seconds: None,
            language: None,
            model_version: None,
        }),
        Err(TranscriptionContractError::TranscriptTooLong)
    );
}

#[test]
fn optional_response_members_preserve_the_schema_nullability() {
    assert!(
        serde_json::from_value::<TranscriptionWire>(serde_json::json!({
            "transcript": "hello",
            "duration_seconds": null
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<TranscriptionWire>(serde_json::json!({
            "transcript": "hello",
            "model_version": null
        }))
        .is_err()
    );
    let wire = serde_json::from_value::<TranscriptionWire>(serde_json::json!({
        "transcript": "hello",
        "language": null
    }))
    .unwrap();
    assert_eq!(wire.language, None);
}
