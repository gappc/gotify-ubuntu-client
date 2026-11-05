//! Resume from suspend and "network is back" signals from the system bus.
//! A WebSocket can sit half-dead after either, so the app reconnects.

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

use crate::app::Event;

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login1 {
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait NetworkManager {
    #[zbus(signal)]
    fn state_changed(&self, state: u32) -> zbus::Result<()>;
}

/// NM_STATE_CONNECTED_GLOBAL
const NM_CONNECTED: u32 = 70;

pub async fn watch(tx: UnboundedSender<Event>) {
    let system = match zbus::Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gotify-tray: no system bus, won't notice suspend/network changes: {e}");
            return;
        }
    };
    let resumed = async {
        let proxy = Login1Proxy::new(&system).await?;
        let mut signals = proxy.receive_prepare_for_sleep().await?;
        while let Some(s) = signals.next().await {
            if matches!(s.args(), Ok(a) if !a.start) {
                let _ = tx.send(Event::Wake);
            }
        }
        zbus::Result::Ok(())
    };
    let network = async {
        let proxy = NetworkManagerProxy::new(&system).await?;
        let mut signals = proxy.receive_state_changed().await?;
        while let Some(s) = signals.next().await {
            if matches!(s.args(), Ok(a) if a.state == NM_CONNECTED) {
                let _ = tx.send(Event::Wake);
            }
        }
        zbus::Result::Ok(())
    };
    let (a, b) = tokio::join!(resumed, network);
    for e in [a, b].into_iter().filter_map(Result::err) {
        eprintln!("gotify-tray: system signal watch ended: {e}");
    }
}
