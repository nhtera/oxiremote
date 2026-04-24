/// Desktop capture, encode, and input injection crate for OxiRemote.
///
/// Provides:
/// - `ScreenCapture` / `CaptureLoop` — xcap-backed frame acquisition
/// - `TileDiff` / `TileEncoder` — xxhash3 tile diff + mozjpeg JPEG encode
/// - `InputInjector` / `InputEvent` — enigo-backed OS input synthesis
/// - `desktop_available()` — platform permission probe (safe at boot)
pub mod capture;
pub mod encode;
pub mod input;
pub mod permissions;

// Flat re-exports used by the agent crate and Phase 04 transport layer.
pub use encode::{EncodedTile, FrameOutput, QualityTier};
pub use input::InputEvent;
pub use permissions::{desktop_available, list_monitors, MonitorInfo};
