/// Desktop capture, encode, and input injection crate for OxiRemote.
///
/// Provides:
/// - `ScreenCapture` / `CaptureLoop` — xcap-backed frame acquisition
/// - `TileDiff` / `TileEncoder` — xxhash3 tile diff + mozjpeg JPEG encode
/// - `InputInjector` / `InputEvent` — enigo-backed OS input synthesis
/// - `desktop_available()` — platform permission probe (safe at boot)
pub mod capture;
pub mod encode;
#[cfg(feature = "h264")]
pub mod encoders;
pub mod h264_format;
pub mod input;
pub mod permissions;
#[cfg(target_os = "macos")]
pub mod sck;

// Flat re-exports used by the agent crate and Phase 04 transport layer.
pub use capture::{primary_scale_factor, RawBgraFrame};
pub use encode::{resize_dims, EncodedTile, FrameOutput, QualityTier, TILE_SIZE};
pub use h264_format::{annexb_to_avcc, avcc_to_annexb, build_avcc, split_annexb};
pub use input::InputEvent;
pub use permissions::{desktop_available, list_monitors, MonitorInfo};
