//! The D-Bus service (`dev.gotifytray.GotifyTray1`) used by the window.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use tokio::sync::mpsc::UnboundedSender;

use crate::app::{Event, ServiceCall};

pub struct Service {
    pub tx: UnboundedSender<Event>,
    pub last_read: Arc<AtomicI64>,
}

impl Service {
    fn send(&self, call: ServiceCall) {
        let _ = self.tx.send(Event::Service(call));
    }
}

#[zbus::interface(name = "dev.gotifytray.GotifyTray1")]
impl Service {
    fn show_window(&self, message_id: i64) {
        self.send(ServiceCall::ShowWindow((message_id > 0).then_some(message_id)));
    }

    fn resync(&self) {
        self.send(ServiceCall::Resync);
    }

    fn reload_config(&self) {
        self.send(ServiceCall::ReloadConfig);
    }

    fn mark_read(&self, up_to_id: i64) {
        self.send(ServiceCall::MarkRead(up_to_id));
    }

    fn last_read(&self) -> i64 {
        self.last_read.load(Ordering::Relaxed)
    }
}
