//! Starts `gotify-tray-window` (next to this binary) and the browser.

use std::path::PathBuf;

use tokio::process::Child;

use crate::app::Launcher;

pub struct ProcessLauncher;

fn window_binary() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("gotify-tray-window")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("gotify-tray-window"))
}

impl Launcher for ProcessLauncher {
    /// GtkApplication forwards a second start to the open window, so this
    /// can simply start another process.
    fn window(&mut self, args: Vec<String>) -> Option<Child> {
        tokio::process::Command::new(window_binary())
            .args(args)
            .spawn()
            .inspect_err(|e| eprintln!("gotify-tray: can't start the window: {e}"))
            .ok()
    }

    fn open_link(&mut self, url: String) {
        if let Err(e) = std::process::Command::new("xdg-open").arg(url).spawn() {
            eprintln!("gotify-tray: can't open link: {e}");
        }
    }
}
