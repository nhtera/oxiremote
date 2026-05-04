// Per-device file-API last-activity tracker.
//
// A lightweight `DashMap<DeviceId, Instant>` updated on every successful file
// API call. The `active_sessions_snapshot` helper in `agent_api` reads this to
// mark devices as "files_active" when their last touch is < 30s ago.
//
// No persistence needed — activity is transient; resets on agent restart.

use std::time::Instant;

use dashmap::DashMap;

/// Active-threshold: a device is considered "files" if it touched the files
/// API within this window.
const FILES_ACTIVE_THRESHOLD_SECS: u64 = 30;

/// Thread-safe map from device_id to the Instant of the last file API call.
pub type FilesActivityMap = DashMap<String, Instant>;

/// Create a new empty activity map. Store one instance behind `Arc` in `AppState`.
pub fn new_map() -> FilesActivityMap {
    DashMap::new()
}

/// Record activity for a device. Call this on every authenticated file API
/// request where the device_id is known.
pub fn touch(map: &FilesActivityMap, device_id: &str) {
    map.insert(device_id.to_string(), Instant::now());
}

/// Returns `true` when `device_id` has a file API touch within the active
/// threshold window.
pub fn is_active(map: &FilesActivityMap, device_id: &str) -> bool {
    map.get(device_id)
        .map(|t| t.elapsed().as_secs() < FILES_ACTIVE_THRESHOLD_SECS)
        .unwrap_or(false)
}
