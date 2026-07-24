//! Target-gated, memory-only native audio capture and WAV encoding.

#![forbid(unsafe_code)]

use heyfood_application::{AudioCapture, AudioCapturePort, BoxFuture, PortError};
use tokio_util::sync::CancellationToken;

/// The package version shared by the native workspace.
pub const VERSION: &str = heyfood_core::VERSION;

#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableAudioCapture;

impl AudioCapturePort for UnavailableAudioCapture {
    fn available(&self) -> bool {
        false
    }

    fn capture(
        &self,
        _stop: CancellationToken,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<AudioCapture, PortError>> {
        Box::pin(async {
            Err(PortError::new(
                "voice_capture_unavailable",
                "native microphone capture is unavailable in this artifact",
            ))
        })
    }
}

#[cfg(all(
    feature = "native-audio",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod native {
    use std::io::Cursor;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, Sample, SampleFormat, SampleRate, SizedSample, SupportedStreamConfig};
    use heyfood_application::{AudioCapture, AudioCapturePort, BoxFuture, PortError};
    use heyfood_core::{
        TRANSCRIPTION_CHANNELS, TRANSCRIPTION_MAX_AUDIO_BYTES, TRANSCRIPTION_MAX_DURATION_SECONDS,
        TRANSCRIPTION_PREFERRED_SAMPLE_RATE_HZ, TRANSCRIPTION_SAMPLE_RATE_MAX_HZ,
        TRANSCRIPTION_SAMPLE_RATE_MIN_HZ, TRANSCRIPTION_SAMPLE_WIDTH_BYTES,
        TRANSCRIPTION_WAV_HEADER_BYTES, transcription_sample_rate_supported,
    };
    use tokio_util::sync::CancellationToken;

    #[derive(Clone, Copy, Debug, Default)]
    pub struct NativeAudioCapture;

    impl NativeAudioCapture {
        pub fn input_device_count(&self) -> Result<usize, PortError> {
            cpal::default_host()
                .input_devices()
                .map(|devices| devices.count())
                .map_err(|_| {
                    PortError::new(
                        "voice_device_query",
                        "microphone devices could not be enumerated",
                    )
                })
        }
    }

    impl AudioCapturePort for NativeAudioCapture {
        fn available(&self) -> bool {
            let host = cpal::default_host();
            host.default_input_device()
                .is_some_and(|device| select_input_config(&device).is_ok())
        }

        fn capture(
            &self,
            stop: CancellationToken,
            cancellation: CancellationToken,
        ) -> BoxFuture<'_, Result<AudioCapture, PortError>> {
            Box::pin(async move {
                tokio::task::spawn_blocking(move || capture_blocking(stop, cancellation))
                    .await
                    .map_err(|_| {
                        PortError::new(
                            "voice_capture_failed",
                            "the microphone capture worker stopped unexpectedly",
                        )
                    })?
            })
        }
    }

    fn capture_blocking(
        stop: CancellationToken,
        cancellation: CancellationToken,
    ) -> Result<AudioCapture, PortError> {
        if cancellation.is_cancelled() {
            return Err(cancelled());
        }
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or_else(|| {
            PortError::new(
                "voice_capture_unavailable",
                "no default microphone input device is available",
            )
        })?;
        let supported = select_input_config(&device)?;
        let sample_rate_hz = supported.sample_rate().0;
        let channels = usize::from(supported.channels());
        let maximum_samples = maximum_pcm_samples(sample_rate_hz);
        let samples = Arc::new(Mutex::new(Vec::<i16>::with_capacity(maximum_samples)));
        let truncated = Arc::new(AtomicBool::new(false));
        let overflowed = Arc::new(AtomicBool::new(false));
        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::I8 => build_stream::<i8>(
                &device,
                &config,
                samples.clone(),
                truncated.clone(),
                overflowed.clone(),
                channels,
                maximum_samples,
            ),
            SampleFormat::I16 => build_stream::<i16>(
                &device,
                &config,
                samples.clone(),
                truncated.clone(),
                overflowed.clone(),
                channels,
                maximum_samples,
            ),
            SampleFormat::I24 => build_stream::<cpal::I24>(
                &device,
                &config,
                samples.clone(),
                truncated.clone(),
                overflowed.clone(),
                channels,
                maximum_samples,
            ),
            SampleFormat::I32 => build_stream::<i32>(
                &device,
                &config,
                samples.clone(),
                truncated.clone(),
                overflowed.clone(),
                channels,
                maximum_samples,
            ),
            SampleFormat::I64 => build_stream::<i64>(
                &device,
                &config,
                samples.clone(),
                truncated.clone(),
                overflowed.clone(),
                channels,
                maximum_samples,
            ),
            SampleFormat::U8 => build_stream::<u8>(
                &device,
                &config,
                samples.clone(),
                truncated.clone(),
                overflowed.clone(),
                channels,
                maximum_samples,
            ),
            SampleFormat::U16 => build_stream::<u16>(
                &device,
                &config,
                samples.clone(),
                truncated.clone(),
                overflowed.clone(),
                channels,
                maximum_samples,
            ),
            SampleFormat::U32 => build_stream::<u32>(
                &device,
                &config,
                samples.clone(),
                truncated.clone(),
                overflowed.clone(),
                channels,
                maximum_samples,
            ),
            SampleFormat::U64 => build_stream::<u64>(
                &device,
                &config,
                samples.clone(),
                truncated.clone(),
                overflowed.clone(),
                channels,
                maximum_samples,
            ),
            SampleFormat::F32 => build_stream::<f32>(
                &device,
                &config,
                samples.clone(),
                truncated.clone(),
                overflowed.clone(),
                channels,
                maximum_samples,
            ),
            SampleFormat::F64 => build_stream::<f64>(
                &device,
                &config,
                samples.clone(),
                truncated.clone(),
                overflowed.clone(),
                channels,
                maximum_samples,
            ),
            _ => Err(PortError::new(
                "voice_sample_format",
                "the microphone sample format is unsupported",
            )),
        }?;
        stream.play().map_err(|_| {
            PortError::new(
                "voice_capture_failed",
                "the microphone stream could not be started",
            )
        })?;
        let started = std::time::Instant::now();
        let hard_limit = Duration::from_secs(TRANSCRIPTION_MAX_DURATION_SECONDS);
        let reached_hard_limit = loop {
            if cancellation.is_cancelled() {
                drop(stream);
                clear_samples(&samples);
                return Err(cancelled());
            }
            if stop.is_cancelled() {
                break false;
            }
            if started.elapsed() >= hard_limit {
                break true;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        drop(stream);
        if reached_hard_limit {
            truncated.store(true, Ordering::Release);
        }
        let pcm = take_samples(&samples)?;
        if pcm.is_empty() {
            return Err(PortError::new(
                "voice_capture_empty",
                "the microphone did not provide any audio samples",
            ));
        }
        let wav_bytes = pcm_i16_to_wav(&pcm, sample_rate_hz)?;
        if wav_bytes.len() > TRANSCRIPTION_MAX_AUDIO_BYTES {
            return Err(PortError::new(
                "voice_capture_too_large",
                "the completed recording exceeds the transcription contract",
            ));
        }
        let duration_millis =
            (u64::try_from(pcm.len()).unwrap_or(u64::MAX) * 1_000) / u64::from(sample_rate_hz);
        Ok(AudioCapture {
            wav_bytes,
            sample_rate_hz,
            duration_millis,
            truncated: truncated.load(Ordering::Acquire),
            overflowed: overflowed.load(Ordering::Acquire),
        })
    }

    fn select_input_config(device: &cpal::Device) -> Result<SupportedStreamConfig, PortError> {
        if let Ok(default) = device.default_input_config()
            && default.channels() > 0
            && transcription_sample_rate_supported(default.sample_rate().0)
            && supported_sample_format(default.sample_format())
        {
            return Ok(default);
        }
        let ranges = device.supported_input_configs().map_err(|_| {
            PortError::new(
                "voice_device_config",
                "the microphone did not publish its supported formats",
            )
        })?;
        let mut fallback = None;
        for range in ranges {
            if range.channels() == 0 || !supported_sample_format(range.sample_format()) {
                continue;
            }
            let minimum = range
                .min_sample_rate()
                .0
                .max(TRANSCRIPTION_SAMPLE_RATE_MIN_HZ);
            let maximum = range
                .max_sample_rate()
                .0
                .min(TRANSCRIPTION_SAMPLE_RATE_MAX_HZ);
            if minimum > maximum {
                continue;
            }
            if (minimum..=maximum).contains(&TRANSCRIPTION_PREFERRED_SAMPLE_RATE_HZ) {
                return Ok(
                    range.with_sample_rate(SampleRate(TRANSCRIPTION_PREFERRED_SAMPLE_RATE_HZ))
                );
            }
            let candidate = range.with_sample_rate(SampleRate(maximum));
            if fallback
                .as_ref()
                .is_none_or(|current: &SupportedStreamConfig| {
                    candidate.channels() < current.channels()
                })
            {
                fallback = Some(candidate);
            }
        }
        fallback.ok_or_else(|| {
            PortError::new(
                "voice_device_config",
                "the microphone has no supported 8–48 kHz input format",
            )
        })
    }

    fn supported_sample_format(format: SampleFormat) -> bool {
        matches!(
            format,
            SampleFormat::I8
                | SampleFormat::I16
                | SampleFormat::I24
                | SampleFormat::I32
                | SampleFormat::I64
                | SampleFormat::U8
                | SampleFormat::U16
                | SampleFormat::U32
                | SampleFormat::U64
                | SampleFormat::F32
                | SampleFormat::F64
        )
    }

    fn build_stream<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        samples: Arc<Mutex<Vec<i16>>>,
        truncated: Arc<AtomicBool>,
        overflowed: Arc<AtomicBool>,
        channels: usize,
        maximum_samples: usize,
    ) -> Result<cpal::Stream, PortError>
    where
        T: SizedSample + Copy + Send + 'static,
        i16: FromSample<T>,
    {
        let stream_overflowed = overflowed.clone();
        device
            .build_input_stream(
                config,
                move |input: &[T], _| {
                    let Ok(mut samples) = samples.try_lock() else {
                        overflowed.store(true, Ordering::Release);
                        return;
                    };
                    for frame in input.chunks(channels) {
                        if samples.len() >= maximum_samples {
                            truncated.store(true, Ordering::Release);
                            break;
                        }
                        if let Some(sample) = frame.first().copied() {
                            samples.push(i16::from_sample(sample));
                        }
                    }
                },
                move |_| stream_overflowed.store(true, Ordering::Release),
                None,
            )
            .map_err(|_| {
                PortError::new(
                    "voice_capture_failed",
                    "the microphone stream could not be opened",
                )
            })
    }

    fn maximum_pcm_samples(sample_rate_hz: u32) -> usize {
        let duration_limit = u64::from(sample_rate_hz) * TRANSCRIPTION_MAX_DURATION_SECONDS;
        let byte_limit = (TRANSCRIPTION_MAX_AUDIO_BYTES - TRANSCRIPTION_WAV_HEADER_BYTES)
            / TRANSCRIPTION_SAMPLE_WIDTH_BYTES;
        usize::try_from(duration_limit)
            .unwrap_or(usize::MAX)
            .min(byte_limit)
    }

    fn pcm_i16_to_wav(pcm: &[i16], sample_rate_hz: u32) -> Result<Vec<u8>, PortError> {
        if !transcription_sample_rate_supported(sample_rate_hz) {
            return Err(PortError::new(
                "voice_sample_rate",
                "the microphone sample rate is outside the transcription contract",
            ));
        }
        let mut bytes = Vec::new();
        {
            let cursor = Cursor::new(&mut bytes);
            let spec = hound::WavSpec {
                channels: TRANSCRIPTION_CHANNELS,
                sample_rate: sample_rate_hz,
                bits_per_sample: u16::try_from(TRANSCRIPTION_SAMPLE_WIDTH_BYTES * 8)
                    .expect("sample width is a fixed u16-compatible constant"),
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::new(cursor, spec).map_err(|_| {
                PortError::new("voice_wav_encode", "the recording could not be encoded")
            })?;
            for sample in pcm {
                writer.write_sample(*sample).map_err(|_| {
                    PortError::new("voice_wav_encode", "the recording could not be encoded")
                })?;
            }
            writer.finalize().map_err(|_| {
                PortError::new("voice_wav_encode", "the recording could not be finalized")
            })?;
        }
        Ok(bytes)
    }

    fn take_samples(samples: &Mutex<Vec<i16>>) -> Result<Vec<i16>, PortError> {
        samples
            .lock()
            .map(|mut samples| std::mem::take(&mut *samples))
            .map_err(|_| {
                PortError::new(
                    "voice_capture_failed",
                    "the microphone sample buffer became unavailable",
                )
            })
    }

    fn clear_samples(samples: &Mutex<Vec<i16>>) {
        if let Ok(mut samples) = samples.lock() {
            samples.fill(0);
            samples.clear();
        }
    }

    fn cancelled() -> PortError {
        PortError::new(
            "voice_capture_cancelled",
            "voice capture was cancelled and discarded",
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn wav_encoding_is_memory_only_mono_and_contract_bounded() {
            let pcm = vec![i16::MIN, 0, i16::MAX];
            let wav = pcm_i16_to_wav(&pcm, TRANSCRIPTION_PREFERRED_SAMPLE_RATE_HZ).unwrap();
            assert_eq!(&wav[..4], b"RIFF");
            assert_eq!(&wav[8..12], b"WAVE");
            assert_eq!(wav.len(), TRANSCRIPTION_WAV_HEADER_BYTES + pcm.len() * 2);
            assert!(wav.len() <= TRANSCRIPTION_MAX_AUDIO_BYTES);
        }

        #[test]
        fn capture_capacity_never_exceeds_duration_or_wire_limits() {
            assert_eq!(
                maximum_pcm_samples(TRANSCRIPTION_SAMPLE_RATE_MAX_HZ),
                usize::try_from(
                    u64::from(TRANSCRIPTION_SAMPLE_RATE_MAX_HZ)
                        * TRANSCRIPTION_MAX_DURATION_SECONDS
                )
                .unwrap()
            );
            assert!(
                maximum_pcm_samples(TRANSCRIPTION_SAMPLE_RATE_MAX_HZ)
                    * TRANSCRIPTION_SAMPLE_WIDTH_BYTES
                    + TRANSCRIPTION_WAV_HEADER_BYTES
                    <= TRANSCRIPTION_MAX_AUDIO_BYTES
            );
        }
    }
}

#[cfg(all(
    feature = "native-audio",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub use native::NativeAudioCapture;
