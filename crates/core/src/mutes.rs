//! Muted monitors and apps: no pop-ups and no red tray icon until the mute
//! ends (after a day, a week, or when unmuted).

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::kuma::parse_kuma;
use crate::model::Message;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MuteTarget {
    /// An Uptime Kuma monitor, by name.
    Monitor(String),
    /// A Gotify application, by id.
    App(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteFor {
    Day,
    Week,
    Forever,
}

impl MuteFor {
    pub const ALL: [MuteFor; 3] = [MuteFor::Day, MuteFor::Week, MuteFor::Forever];

    pub fn label(self) -> &'static str {
        match self {
            MuteFor::Day => "Mute for 1 day",
            MuteFor::Week => "Mute for 1 week",
            MuteFor::Forever => "Mute until unmuted",
        }
    }

    /// Name used over D-Bus.
    pub fn wire(self) -> &'static str {
        match self {
            MuteFor::Day => "day",
            MuteFor::Week => "week",
            MuteFor::Forever => "forever",
        }
    }

    pub fn from_wire(s: &str) -> Option<MuteFor> {
        MuteFor::ALL.into_iter().find(|m| m.wire() == s)
    }

    fn until(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            MuteFor::Day => Some(now + Duration::days(1)),
            MuteFor::Week => Some(now + Duration::days(7)),
            MuteFor::Forever => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Mute {
    target: MuteTarget,
    /// None = until unmuted.
    until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Mutes(Vec<Mute>);

impl Mutes {
    pub fn mute(&mut self, target: MuteTarget, duration: MuteFor, now: DateTime<Utc>) {
        self.unmute(&target);
        self.0.push(Mute { target, until: duration.until(now) });
        self.0.sort_by(|a, b| a.target.cmp(&b.target));
    }

    pub fn unmute(&mut self, target: &MuteTarget) {
        self.0.retain(|m| m.target != *target);
    }

    pub fn is_target_muted(&self, target: &MuteTarget, now: DateTime<Utc>) -> bool {
        self.0.iter().any(|m| m.target == *target && m.until.is_none_or(|u| now < u))
    }

    pub fn is_muted(&self, msg: &Message, now: DateTime<Utc>) -> bool {
        self.is_target_muted(&MuteTarget::App(msg.appid), now)
            || parse_kuma(&msg.message).is_some_and(|e| self.is_target_muted(&MuteTarget::Monitor(e.monitor), now))
    }

    /// Active mutes with their end (None = until unmuted).
    pub fn list(&self, now: DateTime<Utc>) -> Vec<(MuteTarget, Option<DateTime<Utc>>)> {
        self.0.iter().filter(|m| m.until.is_none_or(|u| now < u)).map(|m| (m.target.clone(), m.until)).collect()
    }

    pub fn next_expiry(&self) -> Option<DateTime<Utc>> {
        self.0.iter().filter_map(|m| m.until).min()
    }

    /// Removes ended mutes and returns their targets.
    pub fn take_expired(&mut self, now: DateTime<Utc>) -> Vec<MuteTarget> {
        let (ended, kept): (Vec<Mute>, Vec<Mute>) = self.0.drain(..).partition(|m| m.until.is_some_and(|u| u <= now));
        self.0 = kept;
        ended.into_iter().map(|m| m.target).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 5, 21, 0, 0).unwrap()
    }
    fn kuma(name: &str, appid: i64) -> Message {
        Message { id: 1, appid, message: format!("[{name}] [🔴 Down] x"), title: None, priority: 8, date: now(), extras: None }
    }
    fn web() -> MuteTarget {
        MuteTarget::Monitor("web01".into())
    }

    #[test]
    fn day_and_week_mutes_expire() {
        let mut m = Mutes::default();
        m.mute(web(), MuteFor::Day, now());
        assert!(m.is_target_muted(&web(), now() + Duration::hours(23)));
        assert!(!m.is_target_muted(&web(), now() + Duration::hours(24)));
        m.mute(web(), MuteFor::Week, now());
        assert!(m.is_target_muted(&web(), now() + Duration::days(6)));
        assert!(!m.is_target_muted(&web(), now() + Duration::days(7)));
    }

    #[test]
    fn forever_lasts_until_unmuted() {
        let mut m = Mutes::default();
        m.mute(web(), MuteFor::Forever, now());
        assert!(m.is_target_muted(&web(), now() + Duration::days(400)));
        m.unmute(&web());
        assert!(!m.is_target_muted(&web(), now()));
    }

    #[test]
    fn muting_again_replaces_the_old_mute() {
        let mut m = Mutes::default();
        m.mute(web(), MuteFor::Forever, now());
        m.mute(web(), MuteFor::Day, now());
        assert_eq!(m.list(now()), [(web(), Some(now() + Duration::days(1)))]);
    }

    #[test]
    fn messages_are_muted_by_monitor_or_app() {
        let mut m = Mutes::default();
        m.mute(web(), MuteFor::Day, now());
        m.mute(MuteTarget::App(7), MuteFor::Day, now());
        assert!(m.is_muted(&kuma("web01", 1), now()));
        assert!(!m.is_muted(&kuma("db", 1), now()));
        assert!(m.is_muted(&kuma("db", 7), now()), "whole app muted");
        let mut plain = kuma("x", 1);
        plain.message = "deploy done".into();
        assert!(!m.is_muted(&plain, now()));
    }

    #[test]
    fn expired_mutes_are_reported_once() {
        let mut m = Mutes::default();
        m.mute(web(), MuteFor::Day, now());
        m.mute(MuteTarget::App(7), MuteFor::Forever, now());
        assert_eq!(m.next_expiry(), Some(now() + Duration::days(1)));
        assert!(m.take_expired(now()).is_empty());
        assert_eq!(m.take_expired(now() + Duration::days(2)), [web()]);
        assert!(m.take_expired(now() + Duration::days(3)).is_empty());
        assert_eq!(m.next_expiry(), None);
    }

    #[test]
    fn durations_have_wire_names() {
        assert_eq!(MuteFor::from_wire("day"), Some(MuteFor::Day));
        assert_eq!(MuteFor::from_wire("week"), Some(MuteFor::Week));
        assert_eq!(MuteFor::from_wire("forever"), Some(MuteFor::Forever));
        assert_eq!(MuteFor::from_wire("year"), None);
        assert_eq!(MuteFor::Day.label(), "Mute for 1 day");
        assert_eq!(MuteFor::Forever.label(), "Mute until unmuted");
    }
}
