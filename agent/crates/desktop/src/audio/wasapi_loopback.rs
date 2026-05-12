//! Windows WASAPI loopback capture (phase-02b).
//!
//! Opens the system *output* device via cpal and runs an input stream on it —
//! cpal-Windows auto-flips this path to WASAPI shared-mode loopback, capturing
//! whatever the OS mixer is currently rendering. Rustdesk uses the same
//! `cpal::default_host().default_output_device()` recipe in production, so the
//! kill-switch (phase-02b step 1) is a smoke gate, not a discovery risk.
//!
//! Send-ness: cpal's `Stream` is `!Send` on Windows (WASAPI is COM-backed and
//! sticks to its creator thread). The `AudioCapture` trait requires `Send`, so
//! we keep the stream pinned to a dedicated holder thread; the struct exposed
//! to async code holds only Send-safe handles (tokio `Receiver`, a sync `mpsc`
//! stop signal, and the shared error state).
//!
//! Wire format pinned by `super::desktop_audio_pipeline`: 20 ms / 48 kHz /
//! stereo / interleaved i16. This module owns the format conversion so the
//! pipeline (and the macOS path) can stay symmetric:
//!   1. cpal callback delivers `&[T]` at the device's native rate + channels.
//!   2. We pack to interleaved stereo i16 (mono → duplicate; multi-channel →
//!      L+R only — surround downmix is YAGNI for v1).
//!   3. When we have 20 ms of native-rate stereo, the resampler converts to
//!      48 kHz (passthrough when the device is already 48 kHz).
//!   4. A small "wire buffer" drains exactly 1920-sample frames into the
//!      tokio mpsc the pipeline consumes — absorbs rubato's polynomial slack
//!      so the encoder never sees an odd-sized frame.
//!
//! Mid-session error path: cpal's error callback writes the message to a
//! shared slot. The capture-side mpsc closes naturally when the holder thread
//! exits; `next_frame` then surfaces `CaptureFailed(stored_msg)`, which the
//! pipeline maps to `AudioStopped { reason: "wasapi_error" }`.

#![cfg(target_os = "windows")]

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
// `Sample` is needed in scope so the `f32::from_sample(...)` call resolves —
// the conversion method lives on the `Sample` trait (defaulted, dispatches to
// `FromSample::from_sample_`). Importing only `FromSample` is not enough.
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::resample::{Resampler, TARGET_SAMPLE_RATE};
use super::{AudioCapture, AudioError};

const TARGET_CHANNELS: u16 = 2;
const FRAME_MS: u32 = 20;
/// 1920 interleaved-stereo i16 samples = 20 ms at 48 kHz stereo. Matches
/// `super::opus_encoder::FRAME_SAMPLES_STEREO`.
const TARGET_FRAME_SAMPLES_STEREO: usize = 1920;
/// 8 × 20 ms = 160 ms — matches the macOS SCK audio mpsc capacity in
/// `crate::sck::SckCapture::new_with_audio` so total buffered audio across
/// the platforms stays bounded.
const MPSC_CAP: usize = 8;

/// Cheap "can this binary capture audio?" probe. Enumerates the default
/// output device and reads its config; no stream construction (which would
/// briefly engage WASAPI and surface in audio-routing tools). Cached for the
/// binary lifetime — the underlying device set can change at runtime, but a
/// process restart is the supported recovery path and matches the macOS
/// `sck_audio::probe_supported` contract.
pub fn probe_supported() -> bool {
    static PROBE: OnceLock<bool> = OnceLock::new();
    *PROBE.get_or_init(|| {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            debug!("wasapi probe: no default output device");
            return false;
        };
        match device.default_output_config() {
            Ok(cfg) => {
                debug!(
                    sample_rate = cfg.sample_rate().0,
                    channels = cfg.channels(),
                    sample_format = ?cfg.sample_format(),
                    "wasapi probe: default output device OK"
                );
                true
            }
            Err(e) => {
                debug!(error = %e, "wasapi probe: default_output_config failed");
                false
            }
        }
    })
}

/// Handle exposed to async code. Send-safe by construction (no cpal types).
pub struct WasapiLoopbackCapture {
    rx: mpsc::Receiver<Vec<i16>>,
    err_state: Arc<Mutex<Option<String>>>,
    /// Drop signals the holder thread to release the cpal Stream.
    _stop_tx: std::sync::mpsc::Sender<()>,
}

/// Trait-object factory used by `super::make_default()`.
pub fn make() -> Result<Box<dyn AudioCapture>, AudioError> {
    let (sample_tx, sample_rx) = mpsc::channel::<Vec<i16>>(MPSC_CAP);
    let err_state = Arc::new(Mutex::new(None::<String>));
    let err_state_thread = Arc::clone(&err_state);
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<(), AudioError>>(1);

    // Holder thread owns the cpal Stream because Stream is `!Send` on Windows.
    // The thread blocks on `stop_rx` so the stream stays alive until the
    // capture handle is dropped.
    std::thread::Builder::new()
        .name("wasapi-loopback-holder".into())
        .spawn(move || {
            let stream = match build_stream(sample_tx, Arc::clone(&err_state_thread)) {
                Ok(s) => s,
                Err(e) => {
                    let _ = init_tx.send(Err(e));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let _ = init_tx.send(Err(AudioError::DeviceOpen(format!("stream.play: {e}"))));
                return;
            }
            let _ = init_tx.send(Ok(()));
            // Block until the capture handle drops `stop_tx` (Err) or sends an
            // explicit shutdown (Ok). Either way, the stream drops with this
            // function's stack frame.
            let _ = stop_rx.recv();
            debug!("wasapi_loopback: holder thread exiting; stream dropping");
        })
        .map_err(|e| AudioError::DeviceOpen(format!("spawn holder thread: {e}")))?;

    match init_rx.recv() {
        Ok(Ok(())) => Ok(Box::new(WasapiLoopbackCapture {
            rx: sample_rx,
            err_state,
            _stop_tx: stop_tx,
        })),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(AudioError::DeviceOpen("holder thread died before init".into())),
    }
}

fn build_stream(
    sample_tx: mpsc::Sender<Vec<i16>>,
    err_state: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, AudioError> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| AudioError::DeviceOpen("no default output device".into()))?;
    let supported = device
        .default_output_config()
        .map_err(|e| AudioError::DeviceOpen(format!("default_output_config: {e}")))?;

    let in_rate = supported.sample_rate().0;
    let in_channels = supported.channels();
    let sample_format = supported.sample_format();
    let stream_cfg: StreamConfig = supported.clone().into();

    if in_channels == 0 || in_rate == 0 {
        return Err(AudioError::DeviceOpen(format!(
            "device reports zero channels ({in_channels}) or rate ({in_rate})"
        )));
    }

    let frames_per_chunk_in = ((in_rate as usize) * (FRAME_MS as usize)) / 1000;
    let needs_resample = in_rate != TARGET_SAMPLE_RATE;
    let resampler = if needs_resample {
        Some(Resampler::new(in_rate, frames_per_chunk_in)?)
    } else {
        None
    };

    info!(
        sample_rate = in_rate,
        channels = in_channels,
        sample_format = ?sample_format,
        needs_resample,
        "wasapi_loopback: opening loopback stream"
    );

    // Sample-format dispatch. Windows WASAPI shared-mode is almost always
    // F32; I16/U16 covered for completeness. Unknown formats fail-fast at
    // init rather than silently producing zero-filled audio.
    match sample_format {
        SampleFormat::F32 => build_typed::<f32>(
            &device,
            &stream_cfg,
            in_channels,
            frames_per_chunk_in,
            resampler,
            sample_tx,
            err_state,
        ),
        SampleFormat::I16 => build_typed::<i16>(
            &device,
            &stream_cfg,
            in_channels,
            frames_per_chunk_in,
            resampler,
            sample_tx,
            err_state,
        ),
        SampleFormat::U16 => build_typed::<u16>(
            &device,
            &stream_cfg,
            in_channels,
            frames_per_chunk_in,
            resampler,
            sample_tx,
            err_state,
        ),
        other => Err(AudioError::DeviceOpen(format!(
            "unsupported cpal sample format: {other:?}"
        ))),
    }
}

fn build_typed<T>(
    device: &cpal::Device,
    cfg: &StreamConfig,
    in_channels: u16,
    frames_per_chunk_in: usize,
    mut resampler: Option<Resampler>,
    sample_tx: mpsc::Sender<Vec<i16>>,
    err_state: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let chunk_target = frames_per_chunk_in * 2; // interleaved stereo
    let mut accum: Vec<i16> = Vec::with_capacity(chunk_target);
    let mut wire_buf: Vec<i16> = Vec::with_capacity(TARGET_FRAME_SAMPLES_STEREO * 2);
    let err_cb = Arc::clone(&err_state);

    device
        .build_input_stream(
            cfg,
            move |data: &[T], _info: &cpal::InputCallbackInfo| {
                for frame in data.chunks(in_channels as usize) {
                    // Mono → duplicate; stereo+ → take L+R only. v1 surround
                    // downmix is YAGNI: persona is desktop audio + media
                    // playback, both of which are mixed to stereo by the OS
                    // long before WASAPI sees them.
                    let (l, r) = if in_channels == 1 {
                        let s = f32::from_sample(frame[0]);
                        (s, s)
                    } else {
                        let l = f32::from_sample(frame[0]);
                        let r = f32::from_sample(frame[1]);
                        (l, r)
                    };
                    accum.push(f32_to_i16(l));
                    accum.push(f32_to_i16(r));
                    if accum.len() < chunk_target {
                        continue;
                    }
                    let native_chunk =
                        std::mem::replace(&mut accum, Vec::with_capacity(chunk_target));
                    let resampled = match resampler.as_mut() {
                        Some(rs) => match rs.process_interleaved(&native_chunk) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!(error = %e, "wasapi_loopback: resample failed; skipping chunk");
                                continue;
                            }
                        },
                        None => native_chunk,
                    };
                    wire_buf.extend_from_slice(&resampled);
                    while wire_buf.len() >= TARGET_FRAME_SAMPLES_STEREO {
                        let opus_frame: Vec<i16> =
                            wire_buf.drain(..TARGET_FRAME_SAMPLES_STEREO).collect();
                        if sample_tx.try_send(opus_frame).is_err() {
                            // mpsc full (writer behind) or closed (handle
                            // dropped). Drop-newest on full keeps perceived
                            // latency bounded; closed branch surfaces via
                            // next_frame's None path.
                            debug!("wasapi_loopback: tx full/closed; dropping 20ms frame");
                            break;
                        }
                    }
                }
            },
            move |err| {
                warn!(error = %err, "wasapi_loopback: cpal stream error");
                if let Ok(mut slot) = err_cb.lock()
                    && slot.is_none()
                {
                    *slot = Some(err.to_string());
                }
            },
            None,
        )
        .map_err(|e| AudioError::DeviceOpen(format!("build_input_stream: {e}")))
}

#[inline]
fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

#[async_trait]
impl AudioCapture for WasapiLoopbackCapture {
    async fn next_frame(&mut self) -> Result<Vec<i16>, AudioError> {
        match self.rx.recv().await {
            Some(frame) => Ok(frame),
            None => {
                let msg = self
                    .err_state
                    .lock()
                    .ok()
                    .and_then(|g| g.clone())
                    .unwrap_or_else(|| "loopback channel closed".to_string());
                Err(AudioError::CaptureFailed(msg))
            }
        }
    }

    fn sample_rate(&self) -> u32 {
        TARGET_SAMPLE_RATE
    }

    fn channels(&self) -> u16 {
        TARGET_CHANNELS
    }
}
