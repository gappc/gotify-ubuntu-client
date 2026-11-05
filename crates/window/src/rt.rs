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
            let proxy = gotify_core::ipc::GotifyTrayProxy::new(&conn).await?;
            match call {
                DaemonCall::ReloadConfig => proxy.reload_config().await,
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
    MarkRead(i64),
}

/// The background service's highest read message id (None if unknown).
pub async fn daemon_last_read() -> Option<i64> {
    let conn = zbus::Connection::session().await.ok()?;
    let proxy = gotify_core::ipc::GotifyTrayProxy::new(&conn).await.ok()?;
    proxy.last_read().await.ok().filter(|id| *id > 0)
}
