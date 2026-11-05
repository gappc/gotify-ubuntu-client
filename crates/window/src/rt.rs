//! A small tokio runtime for network calls, with results handed back to the
//! GTK main loop.

use std::future::Future;
use std::sync::OnceLock;

pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().expect("tokio runtime")
    })
}

/// Run `fut` on the runtime and call `done` with its result on the GTK thread.
pub fn spawn_ui<T: Send + 'static>(
    fut: impl Future<Output = T> + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let (tx, rx) = async_channel::bounded(1);
    runtime().spawn(async move {
        let _ = tx.send(fut.await).await;
    });
    gtk::glib::spawn_future_local(async move {
        if let Ok(value) = rx.recv().await {
            done(value);
        }
    });
}

/// Tell the background service something changed (ignored if it isn't running).
pub fn notify_daemon(call: DaemonCall) {
    runtime().spawn(async move {
        let result = async {
            let conn = zbus::Connection::session().await?;
            let proxy = gotify_core::ipc::tray_proxy(&conn).await?;
            match call {
                DaemonCall::ReloadConfig => proxy.reload_config().await,
                DaemonCall::Resync => proxy.resync().await,
                DaemonCall::MarkRead(id) => proxy.mark_read(id).await,
            }
        };
        if let Err(e) = result.await {
            eprintln!("gotify-tray-window: background service not reachable: {e}");
        }
    });
}

#[derive(Debug, Clone, Copy)]
pub enum DaemonCall {
    ReloadConfig,
    /// Messages were deleted: re-check the "down right now" state.
    Resync,
    MarkRead(i64),
}

/// The background service's highest read message id (None if unknown).
pub async fn daemon_last_read() -> Option<i64> {
    let conn = zbus::Connection::session().await.ok()?;
    let proxy = gotify_core::ipc::tray_proxy(&conn).await.ok()?;
    proxy.last_read().await.ok().filter(|id| *id > 0)
}

pub type MuteList = Vec<(gotify_core::mutes::MuteTarget, Option<chrono::DateTime<chrono::Utc>>)>;

/// Active mutes from the background service (empty if it isn't running).
pub async fn daemon_mutes() -> MuteList {
    let fetch = async {
        let conn = zbus::Connection::session().await.ok()?;
        let proxy = gotify_core::ipc::tray_proxy(&conn).await.ok()?;
        serde_json::from_str(&proxy.mutes().await.ok()?).ok()
    };
    fetch.await.unwrap_or_default()
}

/// Mute (duration "day"/"week"/"forever") or unmute (""), then the new list.
pub async fn daemon_set_mute(kind: &str, target: &str, duration: &str) -> Result<MuteList, String> {
    let no_tray = |_| "The tray isn't running, so muting isn't available.".to_string();
    let conn = zbus::Connection::session().await.map_err(no_tray)?;
    let proxy = gotify_core::ipc::tray_proxy(&conn).await.map_err(no_tray)?;
    proxy.set_mute(kind, target, duration).await.map_err(no_tray)?;
    // The service applies it asynchronously; give it a moment before reading back.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    Ok(daemon_mutes().await)
}
