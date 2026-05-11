//! Phase-02a step 1 smoke. Opens an SCStream with `with_captures_audio(true)`,
//! collects ~3 s of audio CMSampleBuffers, and prints format + sanity stats so
//! we can confirm SCK delivers the wire format we expect (48 kHz Float32
//! stereo, ~50 buffers/s for 20 ms framing) before committing to the impl.
//!
//! Run with audio playing on the host (e.g. `say "testing one two three"`,
//! or any music app). Anything else than the kAudioFormatFlagIsFloat path or a
//! non-zero non_zero_buffers count is a discovery worth investigating before
//! step 2.
//!
//! Build/run:
//!   cargo run -p desktop --example sck_audio_smoke
//!
//! macOS-only; the `screencapturekit` crate is unconditional on macOS so no
//! extra feature flag needed. Requires Screen Recording permission for the
//! launching shell (System Settings → Privacy & Security → Screen & System
//! Audio Recording).

#![cfg(target_os = "macos")]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use screencapturekit::cm::CMSampleBuffer;
use screencapturekit::prelude::*;

const SAMPLE_WINDOW_SECS: u64 = 3;

#[derive(Default, Debug, Clone)]
struct AudioStats {
    sample_buffers: usize,
    /// Sum of `num_buffers()` across all sample buffers — typically 1 (interleaved)
    /// or N (planar; one buffer per channel).
    total_audio_buffers: usize,
    /// Sum of `data_byte_size()` across every audio buffer seen. Lets us
    /// compute average bytes/buffer and sanity-check 20 ms framing.
    total_bytes: usize,
    /// Sample buffers where at least one PCM sample was non-zero. < total when
    /// the system is silent or our reinterpretation of the byte buffer as f32
    /// is wrong.
    non_zero_buffers: usize,
    /// Most recently observed format — captured once at first arrival, since
    /// SCK doesn't switch formats mid-stream.
    sample_rate: Option<f64>,
    channel_count: Option<u32>,
    bits_per_channel: Option<u32>,
    bytes_per_frame: Option<u32>,
    is_float: bool,
    format_flags: Option<u32>,
    /// `num_samples()` as reported by CMSampleBuffer (frames per channel,
    /// pre-deinterleave). Captured on first arrival.
    first_num_samples: Option<usize>,
}

struct AudioHandler {
    stats: &'static Mutex<AudioStats>,
    seen: &'static AtomicUsize,
    cap: usize,
    done: &'static AtomicBool,
}

impl SCStreamOutputTrait for AudioHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, kind: SCStreamOutputType) {
        if !matches!(kind, SCStreamOutputType::Audio) {
            return;
        }
        if !sample.is_valid() {
            return;
        }
        if self.seen.load(Ordering::Relaxed) >= self.cap {
            self.done.store(true, Ordering::Relaxed);
            return;
        }

        let mut s = match self.stats.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        s.sample_buffers += 1;
        if s.sample_rate.is_none() {
            if let Some(fd) = sample.format_description() {
                s.sample_rate = fd.audio_sample_rate();
                s.channel_count = fd.audio_channel_count();
                s.bits_per_channel = fd.audio_bits_per_channel();
                s.bytes_per_frame = fd.audio_bytes_per_frame();
                s.is_float = fd.audio_is_float();
                s.format_flags = fd.audio_format_flags();
            }
            s.first_num_samples = Some(sample.num_samples());
        }
        let Some(list) = sample.audio_buffer_list() else {
            self.seen.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let mut had_non_zero = false;
        for buf in list.iter() {
            let bytes = buf.data();
            s.total_audio_buffers += 1;
            s.total_bytes += bytes.len();
            // Non-zero check is byte-level (cheap, format-agnostic). Real impl
            // will reinterpret as f32 then convert to i16; here we only need
            // to confirm the pipeline carries audio, not silence.
            if !had_non_zero && bytes.iter().any(|&b| b != 0) {
                had_non_zero = true;
            }
        }
        if had_non_zero {
            s.non_zero_buffers += 1;
        }
        self.seen.fetch_add(1, Ordering::Relaxed);
    }
}

fn main() -> Result<()> {
    static STATS: Mutex<AudioStats> = Mutex::new(AudioStats {
        sample_buffers: 0,
        total_audio_buffers: 0,
        total_bytes: 0,
        non_zero_buffers: 0,
        sample_rate: None,
        channel_count: None,
        bits_per_channel: None,
        bytes_per_frame: None,
        is_float: false,
        format_flags: None,
        first_num_samples: None,
    });
    static SEEN: AtomicUsize = AtomicUsize::new(0);
    static DONE: AtomicBool = AtomicBool::new(false);

    eprintln!("phase-02a step 1: SCK audio smoke");
    eprintln!("  collecting ~{SAMPLE_WINDOW_SECS}s of audio sample buffers");
    eprintln!("  play audio on this host now (e.g. `say testing` in another shell)");

    // SCK requires a content filter even for audio-only capture. Pick the
    // primary display; SCK delivers global system audio regardless of which
    // display is filtered (audio is not per-display).
    let content = SCShareableContent::get()
        .map_err(|e| anyhow!("SCShareableContent::get failed (Screen Recording perm?): {e}"))?;
    let display = content
        .displays()
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("no displays from SCK"))?;
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&[])
        .build();

    // Configuration: explicit 48 kHz stereo Float32 — the wire format Opus
    // expects. excludes_current_process_audio=true prevents this binary from
    // capturing its own output (would silently feed any tracing log audio if
    // we used `say`). For the smoke we don't need video, but SCK still hands
    // out screen sample buffers; we ignore them in the handler.
    let cap = (SAMPLE_WINDOW_SECS as usize) * 60; // soft cap; ~50/s expected
    let config = SCStreamConfiguration::new()
        .with_captures_audio(true)
        .with_sample_rate(48_000)
        .with_channel_count(2)
        .with_excludes_current_process_audio(true);

    let handler = AudioHandler {
        stats: &STATS,
        seen: &SEEN,
        cap,
        done: &DONE,
    };
    let mut stream = SCStream::new(&filter, &config);
    stream.add_output_handler(handler, SCStreamOutputType::Audio);
    stream
        .start_capture()
        .context("SCStream::start_capture failed (Screen Recording perm?)")?;

    let deadline = std::time::Instant::now() + Duration::from_secs(SAMPLE_WINDOW_SECS);
    while std::time::Instant::now() < deadline && !DONE.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = stream.stop_capture();

    let s = STATS.lock().expect("stats poisoned").clone();
    let elapsed = SAMPLE_WINDOW_SECS as f64;
    let avg_bytes = if s.total_audio_buffers > 0 {
        s.total_bytes as f64 / s.total_audio_buffers as f64
    } else {
        0.0
    };
    eprintln!();
    eprintln!("=== summary ({elapsed:.0} s window) ===");
    eprintln!("  sample buffers received : {}", s.sample_buffers);
    eprintln!(
        "  ≈ rate                  : {:.1}/s (expect ~50/s for 20 ms framing)",
        s.sample_buffers as f64 / elapsed
    );
    eprintln!("  audio buffers (per list): total={} avg={:.2}", s.total_audio_buffers, s.total_audio_buffers as f64 / s.sample_buffers.max(1) as f64);
    eprintln!("  total PCM bytes         : {}", s.total_bytes);
    eprintln!("  avg bytes / buffer      : {:.0}", avg_bytes);
    eprintln!("  non-zero sample buffers : {} / {}", s.non_zero_buffers, s.sample_buffers);
    eprintln!();
    eprintln!("=== format description (first arrival) ===");
    eprintln!("  sample_rate     : {:?}", s.sample_rate);
    eprintln!("  channel_count   : {:?}", s.channel_count);
    eprintln!("  bits_per_channel: {:?}", s.bits_per_channel);
    eprintln!("  bytes_per_frame : {:?}", s.bytes_per_frame);
    eprintln!("  is_float        : {}", s.is_float);
    eprintln!("  format_flags    : {:#010x?}", s.format_flags);
    eprintln!("  num_samples (frames/buf): {:?}", s.first_num_samples);
    eprintln!();

    if s.sample_buffers == 0 {
        return Err(anyhow!(
            "no audio sample buffers arrived — likely missing Screen Recording perm or SCK init issue"
        ));
    }
    if s.non_zero_buffers == 0 {
        eprintln!("WARN: all sample buffers were zero. Was audio actually playing?");
    }
    Ok(())
}
