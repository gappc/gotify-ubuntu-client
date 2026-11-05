//! gotify-tray: keeps one Gotify connection open, shows notifications and a
//! tray icon. The message window is a separate process (gotify-tray-window).

use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::time::Duration;

use futures_util::StreamExt;
use gotify_core::consts::RESYNC_INTERVAL;
use gotify_core::files::Paths;
use gotify_core::ipc::OBJECT_PATH;
use gotify_tray::app::{App, AppDeps, Event};
use gotify_tray::launcher::ProcessLauncher;
use gotify_tray::notify::{DbusBackend, Notifier};
use gotify_tray::service::Service;
use gotify_tray::tray::GotifyTray;
use ksni::TrayMethods;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

fn main() {
    if std::env::args().any(|a| a == "--version") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return;
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
    if let Err(e) = rt.block_on(run()) {
        eprintln!("gotify-tray: {e}");
        std::process::exit(1);
    }
}

async fn run() -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    let backend = DbusBackend::new(&conn).await?;
    let notifier_signals = backend.signals().await?;
    let last_read = Arc::new(AtomicI64::new(0));
    let shared_mutes = Arc::new(std::sync::Mutex::new(gotify_core::mutes::Mutes::default()));
    let deps = AppDeps {
        paths: Paths::from_env(),
        notifier: Notifier::new(backend),
        tokens: Box::new(|url| Box::pin(async move { gotify_core::secret::load_token(&url).await })),
        launcher: Box::new(ProcessLauncher),
        last_read: last_read.clone(),
        sync_delay: Duration::ZERO,
        outage_alert_after: gotify_core::consts::OUTAGE_ALERT_AFTER,
        shared_mutes: shared_mutes.clone(),
    };
    let (app, tx, mut view) = App::new(deps);

    // Serve the interface before taking the name, so a second start that
    // finds the name can always call it.
    conn.object_server().at(OBJECT_PATH, Service { tx: tx.clone(), last_read, mutes: shared_mutes }).await?;
    let owner = matches!(
        conn.request_name_with_flags(gotify_core::ipc::bus_name(), RequestNameFlags::DO_NOT_QUEUE).await,
        Ok(RequestNameReply::PrimaryOwner)
    );
    if !owner {
        // Already running: open its window instead.
        gotify_core::ipc::tray_proxy(&conn).await?.show_window(0).await?;
        return Ok(());
    }

    let (tray_tx, mut tray_rx) = tokio::sync::mpsc::unbounded_channel();
    let tray = GotifyTray::new(tray_tx).spawn().await.map_err(|e| zbus::Error::Failure(format!("tray icon: {e}")))?;
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            while let Some(cmd) = tray_rx.recv().await {
                let _ = tx.send(Event::Tray(cmd));
            }
        });
    }
    tokio::spawn(async move {
        while view.changed().await.is_ok() {
            let v = view.borrow_and_update().clone();
            tray.update(|t| t.view = v).await;
        }
    });
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut signals = std::pin::pin!(notifier_signals);
            while let Some(s) = signals.next().await {
                let _ = tx.send(Event::Notifier(s));
            }
        });
    }
    tokio::spawn(gotify_tray::signals::watch(tx.clone()));
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(RESYNC_INTERVAL);
            every.tick().await;
            loop {
                every.tick().await;
                let _ = tx.send(Event::Tick);
            }
        });
    }
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            let _ = tx.send(Event::Tray(gotify_tray::tray::TrayCommand::Quit));
        });
    }
    app.run().await;
    Ok(())
}
