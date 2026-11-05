//! The D-Bus service (`dev.gotifytray.GotifyTray1`) used by the window.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use tokio::sync::mpsc::UnboundedSender;

use gotify_core::mutes::{MuteFor, MuteTarget, Mutes};

use crate::app::{Event, ServiceCall};

fn parse_target(kind: &str, target: &str) -> Option<MuteTarget> {
    match kind {
        "monitor" if !target.is_empty() => Some(MuteTarget::Monitor(target.to_string())),
        "app" => target.parse().ok().map(MuteTarget::App),
        _ => None,
    }
}

pub struct Service {
    pub tx: UnboundedSender<Event>,
    pub last_read: Arc<AtomicI64>,
    pub mutes: Arc<std::sync::Mutex<Mutes>>,
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

    /// kind: "monitor" or "app"; duration: "day", "week", "forever" or "" to unmute.
    fn set_mute(&self, kind: &str, target: &str, duration: &str) -> zbus::fdo::Result<()> {
        let target = parse_target(kind, target).ok_or_else(|| zbus::fdo::Error::InvalidArgs("unknown mute target".into()))?;
        let duration = match duration {
            "" => None,
            d => Some(MuteFor::from_wire(d).ok_or_else(|| zbus::fdo::Error::InvalidArgs("unknown duration".into()))?),
        };
        self.send(ServiceCall::SetMute(target, duration));
        Ok(())
    }

    /// Active mutes as JSON: [[target, until-or-null], …].
    fn mutes(&self) -> String {
        let list = self.mutes.lock().expect("mutes lock").list(chrono::Utc::now());
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
    }
}
