//! Desktop notifications over org.freedesktop.Notifications.
//!
//! Kuma "Down" notifications are critical (GNOME keeps them on screen);
//! the matching "Up" replaces the Down notification, so no stale banner
//! is left after recovery.

use std::collections::HashMap;

use futures_util::{Stream, StreamExt};
use gotify_core::kuma::{KumaStatus, parse_kuma};
use gotify_core::model::Message;
use gotify_core::rules::{Delivery, delivery_for, display_title};
pub use gotify_core::rules::Urgency;
use gotify_core::text::{escape_markup, first_url};
use zbus::Variant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyRequest {
    pub summary: String,
    pub body: String,
    pub urgency: Urgency,
    pub actions: Vec<(String, String)>,
    pub replaces_id: u32,
}

/// What clicking a notification (or one of its buttons) should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    OpenWindow(Option<i64>),
    OpenLink(String),
    OpenSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifierSignal {
    Action(u32, String),
    Closed(u32),
}

fn default_action() -> (String, String) {
    ("default".into(), "Open".into())
}

pub fn request_for_message(msg: &Message, app_name: Option<&str>, urgency: Urgency) -> NotifyRequest {
    let (summary, body) = match parse_kuma(&msg.message) {
        Some(e) if e.status == KumaStatus::Down => (format!("🔴 {} is down", e.monitor), e.details),
        Some(e) => (format!("✅ {} is up", e.monitor), e.details),
        None => (display_title(msg, app_name), msg.message.clone()),
    };
    let mut actions = vec![default_action()];
    if first_url(&msg.message).is_some() {
        actions.push(("open-link".into(), "Open link".into()));
    }
    NotifyRequest { summary, body: escape_markup(&body), urgency, actions, replaces_id: 0 }
}

pub fn request_for_summary(count: usize, urgency: Urgency, lines: &[String]) -> NotifyRequest {
    NotifyRequest {
        summary: format!("{count} missed messages"),
        body: escape_markup(&lines.join("\n")),
        urgency,
        actions: vec![default_action()],
        replaces_id: 0,
    }
}

pub fn request_for_login_expired() -> NotifyRequest {
    NotifyRequest {
        summary: "Gotify login expired".into(),
        body: "Open Settings from the tray menu to log in again.".into(),
        urgency: Urgency::Normal,
        actions: vec![default_action()],
        replaces_id: 0,
    }
}

/// Sends one notification and returns its id (0 if sending failed).
#[allow(async_fn_in_trait)] // only used inside this crate and its tests
pub trait NotifyBackend {
    async fn notify(&mut self, r: &NotifyRequest) -> u32;
    async fn close(&mut self, id: u32);
}

struct Shown {
    message_id: Option<i64>,
    link: Option<String>,
    settings: bool,
}

pub struct Notifier<B: NotifyBackend> {
    backend: B,
    shown: HashMap<u32, Shown>,
    down_by_monitor: HashMap<String, u32>,
}

impl<B: NotifyBackend> Notifier<B> {
    pub fn new(backend: B) -> Self {
        Notifier { backend, shown: HashMap::new(), down_by_monitor: HashMap::new() }
    }

    #[cfg(test)]
    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub async fn show_message(&mut self, msg: &Message, app_name: Option<&str>) {
        let Delivery::Notify(urgency) = delivery_for(msg) else { return };
        let mut req = request_for_message(msg, app_name, urgency);
        let kuma = parse_kuma(&msg.message);
        if let Some(e) = kuma.as_ref().filter(|e| e.status == KumaStatus::Up) {
            req.replaces_id = self.down_by_monitor.remove(&e.monitor).unwrap_or(0);
        }
        let id = self.backend.notify(&req).await;
        if id == 0 {
            return;
        }
        if let Some(e) = kuma.filter(|e| e.status == KumaStatus::Down) {
            self.down_by_monitor.insert(e.monitor, id);
        }
        let link = first_url(&msg.message).map(str::to_string);
        self.shown.insert(id, Shown { message_id: Some(msg.id), link, settings: false });
    }

    pub async fn show_summary(&mut self, count: usize, urgency: Urgency, lines: &[String]) {
        let id = self.backend.notify(&request_for_summary(count, urgency, lines)).await;
        if id != 0 {
            self.shown.insert(id, Shown { message_id: None, link: None, settings: false });
        }
    }

    pub async fn show_login_expired(&mut self) {
        let id = self.backend.notify(&request_for_login_expired()).await;
        if id != 0 {
            self.shown.insert(id, Shown { message_id: None, link: None, settings: true });
        }
    }

    /// Closes Down banners of monitors whose recovery arrived among missed
    /// messages (those are only summarised, so nothing would replace them).
    pub async fn close_recovered(&mut self, missed: &[Message]) {
        for msg in missed {
            let Some(e) = parse_kuma(&msg.message).filter(|e| e.status == KumaStatus::Up) else { continue };
            if let Some(id) = self.down_by_monitor.remove(&e.monitor) {
                self.backend.close(id).await;
                self.shown.remove(&id);
            }
        }
    }

    pub fn on_closed(&mut self, id: u32) {
        self.shown.remove(&id);
        self.down_by_monitor.retain(|_, v| *v != id);
    }

    pub fn on_action(&mut self, id: u32, key: &str) -> Option<Target> {
        let shown = self.shown.get(&id)?;
        match key {
            "open-link" => shown.link.clone().map(Target::OpenLink),
            _ if shown.settings => Some(Target::OpenSettings),
            _ => Some(Target::OpenWindow(shown.message_id)),
        }
    }
}

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Variant<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}

pub struct DbusBackend {
    proxy: NotificationsProxy<'static>,
}

impl DbusBackend {
    pub async fn new(conn: &zbus::Connection) -> zbus::Result<DbusBackend> {
        Ok(DbusBackend { proxy: NotificationsProxy::new(conn).await? })
    }

    pub async fn signals(&self) -> zbus::Result<impl Stream<Item = NotifierSignal> + use<>> {
        let actions = self.proxy.receive_action_invoked().await?.filter_map(|s| async move {
            let a = s.args().ok()?;
            Some(NotifierSignal::Action(a.id, a.action_key))
        });
        let closed = self.proxy.receive_notification_closed().await?.filter_map(|s| async move {
            Some(NotifierSignal::Closed(s.args().ok()?.id))
        });
        Ok(futures_util::stream::select(actions, closed))
    }
}

impl NotifyBackend for DbusBackend {
    async fn notify(&mut self, r: &NotifyRequest) -> u32 {
        let actions: Vec<&str> = r.actions.iter().flat_map(|(k, l)| [k.as_str(), l.as_str()]).collect();
        let urgency: u8 = match r.urgency {
            Urgency::Low => 0,
            Urgency::Normal => 1,
            Urgency::Critical => 2,
        };
        let hints = HashMap::from([("urgency", Variant::from(urgency)), ("desktop-entry", Variant::from("gotify-tray"))]);
        match self.proxy.notify("Gotify", r.replaces_id, "", &r.summary, &r.body, &actions, hints, -1).await {
            Ok(id) => id,
            Err(e) => {
                eprintln!("gotify-tray: notification failed: {e}");
                0
            }
        }
    }

    async fn close(&mut self, id: u32) {
        if let Err(e) = self.proxy.close_notification(id).await {
            eprintln!("gotify-tray: closing notification failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gotify_core::model::Message;

    fn m(id: i64, text: &str) -> Message {
        Message { id, appid: 1, message: text.into(), title: Some("Uptime-Kuma".into()), priority: 8, date: Utc::now(), extras: None }
    }

    #[derive(Default)]
    struct Fake {
        next_id: u32,
        sent: Vec<NotifyRequest>,
        closed: Vec<u32>,
    }
    impl NotifyBackend for Fake {
        async fn notify(&mut self, r: &NotifyRequest) -> u32 {
            self.sent.push(r.clone());
            if r.replaces_id != 0 {
                return r.replaces_id;
            }
            self.next_id += 1;
            self.next_id + 6 // first id is 7
        }
        async fn close(&mut self, id: u32) {
            self.closed.push(id);
        }
    }

    #[tokio::test]
    async fn recovery_in_summary_closes_down_banner() {
        let mut n = Notifier::new(Fake::default());
        n.show_message(&m(1, "[web01] [🔴 Down] x"), None).await;
        n.show_message(&m(2, "[db] [🔴 Down] x"), None).await;
        // web01 recovered while disconnected; it only shows up in a summary.
        n.close_recovered(&[m(5, "[web01] [✅ Up] ok"), m(4, "deploy")]).await;
        assert_eq!(n.backend().closed, [7]);
        assert_eq!(n.on_action(7, "default"), None);
        // db's banner is untouched and still replaceable.
        n.show_message(&m(6, "[db] [✅ Up] ok"), None).await;
        assert_eq!(n.backend().sent.last().unwrap().replaces_id, 8);
    }

    #[test]
    fn kuma_down_request() {
        let r = request_for_message(&m(1, "[web01] [🔴 Down] timeout"), Some("Kuma"), Urgency::Critical);
        assert_eq!(r.summary, "🔴 web01 is down");
        assert_eq!(r.body, "timeout");
        assert_eq!(r.urgency, Urgency::Critical);
        assert_eq!(r.actions, [("default".to_string(), "Open".to_string())]);
    }

    #[test]
    fn kuma_up_request() {
        let r = request_for_message(&m(1, "[web01] [✅ Up] 200 - OK"), None, Urgency::Normal);
        assert_eq!(r.summary, "✅ web01 is up");
        assert_eq!(r.body, "200 - OK");
    }

    #[tokio::test]
    async fn link_adds_action() {
        let mut n = Notifier::new(Fake::default());
        let mut msg = m(3, "Deploy failed https://ci/x");
        msg.title = Some("Deploy".into());
        n.show_message(&msg, None).await;
        let sent = &n.backend().sent[0];
        assert_eq!(sent.summary, "Deploy");
        assert!(sent.actions.contains(&("open-link".to_string(), "Open link".to_string())));
        assert_eq!(n.on_action(7, "open-link"), Some(Target::OpenLink("https://ci/x".into())));
        assert_eq!(n.on_action(7, "default"), Some(Target::OpenWindow(Some(3))));
        assert_eq!(n.on_action(99, "default"), None);
    }

    #[test]
    fn body_is_escaped() {
        let mut msg = m(1, "a < b & c");
        msg.title = Some("T".into());
        assert_eq!(request_for_message(&msg, None, Urgency::Normal).body, "a &lt; b &amp; c");
    }

    #[tokio::test]
    async fn up_replaces_down() {
        let mut n = Notifier::new(Fake::default());
        n.show_message(&m(1, "[web01] [🔴 Down] x"), None).await;
        n.show_message(&m(2, "[web01] [✅ Up] y"), None).await;
        n.show_message(&m(3, "[web01] [✅ Up] z"), None).await;
        let ids: Vec<_> = n.backend().sent.iter().map(|r| r.replaces_id).collect();
        assert_eq!(ids, [0, 7, 0]);
        // The replaced notification now opens the Up message.
        assert_eq!(n.on_action(7, "default"), Some(Target::OpenWindow(Some(2))));
    }

    #[tokio::test]
    async fn closed_down_is_not_replaced() {
        let mut n = Notifier::new(Fake::default());
        n.show_message(&m(1, "[web01] [🔴 Down] x"), None).await;
        n.on_closed(7);
        n.show_message(&m(2, "[web01] [✅ Up] y"), None).await;
        assert_eq!(n.backend().sent[1].replaces_id, 0);
        assert_eq!(n.on_action(7, "default"), None);
    }

    #[tokio::test]
    async fn silent_messages_are_not_shown() {
        let mut n = Notifier::new(Fake::default());
        let mut msg = m(1, "quiet");
        msg.priority = 0;
        n.show_message(&msg, None).await;
        assert!(n.backend().sent.is_empty());
    }

    #[tokio::test]
    async fn summary_text() {
        let mut n = Notifier::new(Fake::default());
        n.show_summary(5, Urgency::Critical, &["a: b".into(), "c: d".into()]).await;
        let r = &n.backend().sent[0];
        assert_eq!(r.summary, "5 missed messages");
        assert_eq!(r.body, "a: b\nc: d");
        assert_eq!(r.urgency, Urgency::Critical);
        assert_eq!(n.on_action(7, "default"), Some(Target::OpenWindow(None)));
    }

    #[tokio::test]
    async fn login_expired_opens_settings() {
        let mut n = Notifier::new(Fake::default());
        n.show_login_expired().await;
        assert_eq!(n.backend().sent[0].summary, "Gotify login expired");
        assert_eq!(n.backend().sent[0].body, "Open Settings from the tray menu to log in again.");
        assert_eq!(n.on_action(7, "default"), Some(Target::OpenSettings));
    }
}
