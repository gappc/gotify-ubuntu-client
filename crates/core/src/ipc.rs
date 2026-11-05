//! D-Bus interface the background service offers to the window process.

pub const BUS_NAME: &str = "dev.gotifytray.GotifyTray";
pub const OBJECT_PATH: &str = "/dev/gotifytray/GotifyTray";
pub const INTERFACE: &str = "dev.gotifytray.GotifyTray1";
pub const WINDOW_APP_ID: &str = "dev.gotifytray.GotifyTray.Window";

/// The service's bus name; `GOTIFY_TRAY_BUS_NAME` overrides it so test
/// copies never talk to the user's running tray.
pub fn bus_name() -> String {
    std::env::var("GOTIFY_TRAY_BUS_NAME").unwrap_or_else(|_| BUS_NAME.to_string())
}

/// A proxy for the running background service (honours `bus_name()`).
pub async fn tray_proxy(conn: &zbus::Connection) -> zbus::Result<GotifyTrayProxy<'static>> {
    GotifyTrayProxy::builder(conn).destination(bus_name()).build().await
}

#[zbus::proxy(
    interface = "dev.gotifytray.GotifyTray1",
    default_service = "dev.gotifytray.GotifyTray",
    default_path = "/dev/gotifytray/GotifyTray"
)]
pub trait GotifyTray {
    /// Open the message window, optionally at a message (0 = none).
    fn show_window(&self, message_id: i64) -> zbus::Result<()>;
    /// Re-read messages from the server (after deletions in the window).
    fn resync(&self) -> zbus::Result<()>;
    /// Login or logout happened: reload config and token.
    fn reload_config(&self) -> zbus::Result<()>;
    fn mark_read(&self, up_to_id: i64) -> zbus::Result<()>;
    /// Highest read message id (0 = none).
    fn last_read(&self) -> zbus::Result<i64>;
    /// kind: "monitor" or "app"; duration: "day", "week", "forever" or "" to unmute.
    fn set_mute(&self, kind: &str, target: &str, duration: &str) -> zbus::Result<()>;
    /// Active mutes as JSON.
    fn mutes(&self) -> zbus::Result<String>;
}
