//! What to show after missed messages, and the unread count.

use crate::kuma::parse_kuma;
use crate::model::Message;
use crate::rules::{Delivery, Urgency, delivery_for, display_title};

#[derive(Debug, Clone, PartialEq)]
pub enum MissedNotice {
    None,
    Single(Message),
    Summary { count: usize, urgency: Urgency, lines: Vec<String> },
}

const SUMMARY_LINES: usize = 3;

/// `newest_first` may contain already-notified messages; only ids above
/// `last_notified` count. On the first run (`None`) nothing is notified.
pub fn missed_notice(
    last_notified: Option<i64>,
    newest_first: &[Message],
    app_name: impl Fn(i64) -> Option<String>,
) -> (MissedNotice, Option<i64>) {
    let newest = newest_first.iter().map(|m| m.id).max();
    let new_last = match (last_notified, newest) {
        (Some(a), Some(b)) => Some(a.max(b)),
        // First run on an empty server: start at 0 so live messages count.
        (None, None) => Some(0),
        (a, b) => a.or(b),
    };
    let Some(last) = last_notified else { return (MissedNotice::None, new_last) };

    let mut missed: Vec<(&Message, Urgency)> = newest_first
        .iter()
        .filter(|m| m.id > last)
        .filter_map(|m| match delivery_for(m) {
            Delivery::Notify(u) => Some((m, u)),
            Delivery::Silent => None,
        })
        .collect();
    missed.sort_by_key(|(m, _)| std::cmp::Reverse(m.id));

    let notice = match missed.as_slice() {
        [] => MissedNotice::None,
        [(m, _)] => MissedNotice::Single((*m).clone()),
        _ => {
            let urgency = missed.iter().map(|(_, u)| *u).max().unwrap_or(Urgency::Normal);
            let mut lines: Vec<String> = missed
                .iter()
                .take(SUMMARY_LINES)
                .map(|(m, _)| {
                    let text = parse_kuma(&m.message).map_or_else(|| m.message.clone(), |e| e.details);
                    format!("{}: {}", display_title(m, app_name(m.appid).as_deref()), text)
                })
                .collect();
            if missed.len() > SUMMARY_LINES {
                lines.push(format!("…and {} more", missed.len() - SUMMARY_LINES));
            }
            MissedNotice::Summary { count: missed.len(), urgency, lines }
        }
    };
    (notice, new_last)
}

/// The server's newest id is below our markers: it was rebuilt or replaced,
/// so the markers must start over (else new messages would be ignored).
pub fn markers_stale(last_notified: Option<i64>, newest: Option<i64>) -> bool {
    matches!((last_notified, newest), (Some(last), Some(newest)) if newest < last)
}

pub fn unread_count(last_read: Option<i64>, ids: impl IntoIterator<Item = i64>) -> usize {
    match last_read {
        None => 0,
        Some(last) => ids.into_iter().filter(|id| *id > last).count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn m(id: i64, priority: i64, text: &str) -> Message {
        Message { id, appid: 1, message: text.into(), title: Some("Deploy".into()), priority, date: Utc.timestamp_opt(id * 60, 0).unwrap(), extras: None }
    }
    fn names(_: i64) -> Option<String> {
        Some("CI".into())
    }

    #[test]
    fn first_run_notifies_nothing() {
        let (notice, last) = missed_notice(None, &[m(5, 8, "a"), m(4, 8, "b")], names);
        assert_eq!(notice, MissedNotice::None);
        assert_eq!(last, Some(5));
    }

    #[test]
    fn first_run_with_no_messages_starts_marker() {
        // An empty server must still start the marker, or live messages
        // would never be notified.
        assert_eq!(missed_notice(None, &[], names), (MissedNotice::None, Some(0)));
    }

    #[test]
    fn markers_ahead_of_server_are_stale() {
        // Server rebuilt or switched: its newest id is below what we saw.
        assert!(markers_stale(Some(300), Some(12)));
        assert!(!markers_stale(Some(12), Some(300)));
        assert!(!markers_stale(None, Some(3)));
        assert!(!markers_stale(Some(5), None));
    }

    #[test]
    fn nothing_new() {
        let (notice, last) = missed_notice(Some(5), &[m(5, 8, "a")], names);
        assert_eq!(notice, MissedNotice::None);
        assert_eq!(last, Some(5));
    }

    #[test]
    fn single_missed() {
        let (notice, last) = missed_notice(Some(4), &[m(5, 8, "a"), m(4, 8, "b")], names);
        assert_eq!(notice, MissedNotice::Single(m(5, 8, "a")));
        assert_eq!(last, Some(5));
    }

    #[test]
    fn summary_of_five() {
        let msgs: Vec<_> = (6..=10).rev().map(|i| m(i, 5, &format!("msg {i}"))).collect();
        let (notice, last) = missed_notice(Some(5), &msgs, names);
        let MissedNotice::Summary { count, urgency, lines } = notice else { panic!("{notice:?}") };
        assert_eq!(count, 5);
        assert_eq!(urgency, Urgency::Normal);
        assert_eq!(lines, ["Deploy: msg 10", "Deploy: msg 9", "Deploy: msg 8", "…and 2 more"]);
        assert_eq!(last, Some(10));
    }

    #[test]
    fn summary_is_critical_if_any_down() {
        let msgs = [m(3, 1, "low"), m(2, 1, "[web01] [🔴 Down] timeout")];
        let (notice, _) = missed_notice(Some(1), &msgs, names);
        let MissedNotice::Summary { urgency, lines, .. } = notice else { panic!() };
        assert_eq!(urgency, Urgency::Critical);
        assert_eq!(lines[1], "web01: timeout");
    }

    #[test]
    fn silent_messages_not_counted_but_advance_last_notified() {
        let (notice, last) = missed_notice(Some(1), &[m(3, 0, "quiet"), m(2, 5, "loud")], names);
        assert!(matches!(notice, MissedNotice::Single(ref s) if s.id == 2));
        assert_eq!(last, Some(3));
    }

    #[test]
    fn unread_counts_newer_ids() {
        assert_eq!(unread_count(Some(5), [7, 6, 5, 4]), 2);
    }

    #[test]
    fn unread_none_is_zero() {
        assert_eq!(unread_count(None, [7, 6]), 0);
    }

}
