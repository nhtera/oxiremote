/// Desktop capture, encode, and input injection crate for OxiRemote.
///
/// Provides:
/// - `ScreenCapture` / `CaptureLoop` — xcap-backed frame acquisition
/// - `TileDiff` / `TileEncoder` — xxhash3 tile diff + mozjpeg JPEG encode
/// - `InputInjector` / `InputEvent` — enigo-backed OS input synthesis
/// - `desktop_available()` — platform permission probe (safe at boot)
pub mod audio;
pub mod capture;
pub mod encode;
// `encoders` module hosts H.264 (h264), VP9 (vp9), and AV1 (av1) backends.
// Each backend submodule is itself feature-gated, so a build that enables
// only one encoder still compiles cleanly.
#[cfg(any(feature = "h264", feature = "vp9", feature = "av1"))]
pub mod encoders;
pub mod h264_format;
pub mod input;
pub mod macos_lock_observer;
pub mod macos_stay_awake;
pub mod permissions;
#[cfg(target_os = "macos")]
pub mod sck;
#[cfg(target_os = "windows")]
pub mod cursor_windows;

// Flat re-exports used by the agent crate and Phase 04 transport layer.
pub use capture::{frame_interval, primary_monitor_origin, primary_scale_factor, DirtyRect, RawBgraFrame};
pub use encode::{resize_dims, EncodedTile, FrameOutput, QualityTier, TILE_SIZE};
pub use h264_format::{annexb_to_avcc, avcc_to_annexb, build_avcc, split_annexb};
pub use input::InputEvent;
pub use permissions::{desktop_available, list_monitors, MonitorInfo};
