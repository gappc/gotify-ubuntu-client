//! D-Bus interface the background service offers to the window process.

pub const BUS_NAME: &str = "dev.gotifytray.GotifyTray";
pub const OBJECT_PATH: &str = "/dev/gotifytray/GotifyTray";
pub const INTERFACE: &str = "dev.gotifytray.GotifyTray1";
pub const WINDOW_APP_ID: &str = "dev.gotifytray.GotifyTray.Window";

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
}
