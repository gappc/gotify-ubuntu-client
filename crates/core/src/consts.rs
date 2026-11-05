//! Fixed values from the design (see docs/superpowers/specs).

use std::time::Duration;

/// Gotify 3 rejects `limit` values above 200.
pub const PAGE_LIMIT: u32 = 200;
pub const BACKOFF_START: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// No frame (not even a ping) for this long means the stream is dead.
pub const STREAM_SILENCE_TIMEOUT: Duration = Duration::from_secs(100);
pub const RESYNC_INTERVAL: Duration = Duration::from_secs(15 * 60);
pub const MENU_RESYNC_DEBOUNCE: Duration = Duration::from_secs(10);
pub const KEYRING_RETRY: Duration = Duration::from_secs(30);
/// Not connected for this long raises the "Can't reach Gotify" alert.
pub const OUTAGE_ALERT_AFTER: Duration = Duration::from_secs(5 * 60);
