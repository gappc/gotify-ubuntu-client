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

/// Starts a helper and waits for it in the background (so it doesn't stay
/// around as a zombie). The task yields whether it succeeded.
pub fn spawn_reaped(program: &str, args: &[&str]) -> std::io::Result<tokio::task::JoinHandle<bool>> {
    let mut child = tokio::process::Command::new(program).args(args).spawn()?;
    Ok(tokio::spawn(async move { child.wait().await.is_ok_and(|s| s.success()) }))
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
        if let Err(e) = spawn_reaped("xdg-open", &[url.as_str()]) {
            eprintln!("gotify-tray: can't open link: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn started_helpers_are_waited_for() {
        // The returned task waits for the child, so no zombie is left behind.
        let done = spawn_reaped("true", &[]).unwrap();
        assert!(done.await.unwrap());
        assert!(!spawn_reaped("false", &[]).unwrap().await.unwrap());
    }
}
