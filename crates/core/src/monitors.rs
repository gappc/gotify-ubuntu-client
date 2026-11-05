//! "Down right now" state per Uptime Kuma monitor.
//!
//! Rule from the design: deletions only ever clear a monitor. A monitor is
//! down only if its newest known event is Down and that Down message still
//! exists on the server.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::kuma::KumaStatus;
use crate::kuma::parse_kuma;
use crate::model::Message;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorRecord {
    pub status: KumaStatus,
    pub message_id: i64,
    pub since: DateTime<Utc>,
}

pub type Monitors = BTreeMap<String, MonitorRecord>;

fn record_of(msg: &Message) -> Option<(String, MonitorRecord)> {
    let event = parse_kuma(&msg.message)?;
    Some((event.monitor, MonitorRecord { status: event.status, message_id: msg.id, since: msg.date }))
}

pub fn apply_live(monitors: &mut Monitors, msg: &Message) {
    let Some((name, new)) = record_of(msg) else { return };
    match monitors.get(&name) {
        Some(old) if old.message_id >= new.message_id => {}
        _ => {
            monitors.insert(name, new);
        }
    }
}

pub fn resync(known: &Monitors, server: &[Message]) -> Monitors {
    let server_ids: HashSet<i64> = server.iter().map(|m| m.id).collect();
    let mut newest: HashMap<String, MonitorRecord> = HashMap::new();
    for (name, rec) in server.iter().filter_map(record_of) {
        match newest.get(&name) {
            Some(old) if old.message_id >= rec.message_id => {}
            _ => {
                newest.insert(name, rec);
            }
        }
    }
    let names: HashSet<&String> = known.keys().chain(newest.keys()).collect();
    names
        .into_iter()
        .map(|name| {
            let winner = match (known.get(name), newest.get(name)) {
                (Some(k), Some(s)) if k.message_id >= s.message_id => k.clone(),
                (_, Some(s)) => s.clone(),
                (Some(k), None) => k.clone(),
                (None, None) => unreachable!("name comes from one of the maps"),
            };
            let mut rec = winner;
            if rec.status == KumaStatus::Down && !server_ids.contains(&rec.message_id) {
                rec.status = KumaStatus::Up;
            }
            (name.clone(), rec)
        })
        .collect()
}

/// For monitors that will never send "Up" again (deleted or renamed in Kuma).
pub fn clear(monitors: &mut Monitors, name: &str) {
    if let Some(rec) = monitors.get_mut(name) {
        rec.status = KumaStatus::Up;
    }
}

pub fn down_list(monitors: &Monitors) -> Vec<(&str, &MonitorRecord)> {
    let mut list: Vec<_> = monitors
        .iter()
        .filter(|(_, r)| r.status == KumaStatus::Down)
        .map(|(n, r)| (n.as_str(), r))
        .collect();
    list.sort_by_key(|(_, r)| r.since);
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn kuma(id: i64, name: &str, status: &str) -> Message {
        Message {
            id,
            appid: 1,
            message: format!("[{name}] [{status}] details"),
            title: Some("Uptime-Kuma".into()),
            priority: 8,
            date: Utc.timestamp_opt(id * 60, 0).unwrap(),
            extras: None,
        }
    }
    fn down(id: i64, name: &str) -> Message {
        kuma(id, name, "🔴 Down")
    }
    fn up(id: i64, name: &str) -> Message {
        kuma(id, name, "✅ Up")
    }
    fn record(status: KumaStatus, id: i64) -> MonitorRecord {
        MonitorRecord { status, message_id: id, since: Utc.timestamp_opt(id * 60, 0).unwrap() }
    }
    fn known(status: KumaStatus, id: i64) -> Monitors {
        Monitors::from([("web".to_string(), record(status, id))])
    }

    #[test]
    fn first_run_takes_server_state() {
        let m = resync(&Monitors::new(), &[down(1, "web")]);
        assert_eq!(m["web"], record(KumaStatus::Down, 1));
    }

    #[test]
    fn up_after_down() {
        let m = resync(&Monitors::new(), &[up(2, "web"), down(1, "web")]);
        assert_eq!(m["web"].status, KumaStatus::Up);
    }

    #[test]
    fn deleted_down_clears() {
        let m = resync(&known(KumaStatus::Down, 3), &[up(2, "web"), down(1, "web")]);
        assert_eq!(m["web"].status, KumaStatus::Up);
        assert_eq!(m["web"].message_id, 3);
    }

    #[test]
    fn deleted_up_does_not_revive() {
        let m = resync(&known(KumaStatus::Up, 2), &[down(1, "web")]);
        assert_eq!(m["web"], record(KumaStatus::Up, 2));
    }

    #[test]
    fn still_down() {
        let m = resync(&known(KumaStatus::Down, 1), &[down(1, "web")]);
        assert_eq!(m["web"], record(KumaStatus::Down, 1));
    }

    #[test]
    fn newer_up_on_server_wins() {
        let m = resync(&known(KumaStatus::Down, 1), &[up(5, "web"), down(1, "web")]);
        assert_eq!(m["web"], record(KumaStatus::Up, 5));
    }

    #[test]
    fn clear_survives_resync_until_newer_message() {
        let mut m = known(KumaStatus::Down, 1);
        clear(&mut m, "web");
        let mut m = resync(&m, &[down(1, "web")]);
        assert_eq!(m["web"].status, KumaStatus::Up);
        apply_live(&mut m, &down(7, "web"));
        assert_eq!(m["web"], record(KumaStatus::Down, 7));
    }

    #[test]
    fn apply_live_ignores_older() {
        let mut m = known(KumaStatus::Up, 5);
        apply_live(&mut m, &down(4, "web"));
        assert_eq!(m["web"], record(KumaStatus::Up, 5));
    }

    #[test]
    fn non_kuma_ignored() {
        let mut hello = up(1, "web");
        hello.message = "hello".into();
        assert!(resync(&Monitors::new(), &[hello.clone()]).is_empty());
        let mut m = Monitors::new();
        apply_live(&mut m, &hello);
        assert!(m.is_empty());
    }

    #[test]
    fn down_list_sorted() {
        let m = resync(&Monitors::new(), &[down(9, "b"), up(8, "c"), down(3, "a")]);
        let names: Vec<_> = down_list(&m).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["a", "b"]);
    }
}
