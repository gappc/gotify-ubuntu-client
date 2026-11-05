//! How a message is delivered as a desktop notification, and its title.

use crate::kuma::{KumaStatus, parse_kuma};
use crate::model::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Silent,
    Notify(Urgency),
}

/// Kuma Down/Up decide on their own (Kuma sends both with one priority);
/// everything else follows the Gotify Android app's priority ranges.
pub fn delivery_for(msg: &Message) -> Delivery {
    if let Some(event) = parse_kuma(&msg.message) {
        return Delivery::Notify(match event.status {
            KumaStatus::Down => Urgency::Critical,
            KumaStatus::Up => Urgency::Normal,
        });
    }
    match msg.priority {
        p if p >= 8 => Delivery::Notify(Urgency::Critical),
        4..=7 => Delivery::Notify(Urgency::Normal),
        1..=3 => Delivery::Notify(Urgency::Low),
        _ => Delivery::Silent,
    }
}

pub fn display_title(msg: &Message, app_name: Option<&str>) -> String {
    if let Some(event) = parse_kuma(&msg.message) {
        return event.monitor;
    }
    match msg.title.as_deref() {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => app_name.unwrap_or("Gotify").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Message;

    fn msg(priority: i64, text: &str) -> Message {
        Message {
            id: 1,
            appid: 1,
            message: text.into(),
            title: Some("Uptime-Kuma".into()),
            priority,
            date: chrono::Utc::now(),
            extras: None,
        }
    }

    #[test]
    fn kuma_down_is_critical_even_at_low_priority() {
        assert_eq!(delivery_for(&msg(2, "[web01] [🔴 Down] x")), Delivery::Notify(Urgency::Critical));
    }

    #[test]
    fn kuma_up_is_normal_even_at_high_priority() {
        assert_eq!(delivery_for(&msg(8, "[web01] [✅ Up] x")), Delivery::Notify(Urgency::Normal));
    }

    #[test]
    fn priority_ranges() {
        let d = |p| delivery_for(&msg(p, "deploy done"));
        assert_eq!(d(10), Delivery::Notify(Urgency::Critical));
        assert_eq!(d(8), Delivery::Notify(Urgency::Critical));
        assert_eq!(d(11), Delivery::Notify(Urgency::Critical));
        assert_eq!(d(7), Delivery::Notify(Urgency::Normal));
        assert_eq!(d(4), Delivery::Notify(Urgency::Normal));
        assert_eq!(d(3), Delivery::Notify(Urgency::Low));
        assert_eq!(d(1), Delivery::Notify(Urgency::Low));
        assert_eq!(d(0), Delivery::Silent);
        assert_eq!(d(-1), Delivery::Silent);
    }

    #[test]
    fn title_uses_monitor_name() {
        assert_eq!(display_title(&msg(8, "[web01] [🔴 Down] x"), Some("Kuma")), "web01");
    }

    #[test]
    fn title_falls_back() {
        let mut m = msg(5, "deploy done");
        m.title = Some("Deploy".into());
        assert_eq!(display_title(&m, Some("CI")), "Deploy");
        m.title = None;
        assert_eq!(display_title(&m, Some("CI")), "CI");
        m.title = Some(String::new());
        assert_eq!(display_title(&m, Some("CI")), "CI");
        assert_eq!(display_title(&m, None), "Gotify");
    }
}
