//! "Can't reach Gotify" alert: raised once when the connection has been down
//! for a while, cleared when it's back. The caller owns the clock: it schedules
//! a check for the returned sequence number and calls `alert_due` when it fires.

#[derive(Debug, Default)]
pub struct Outage {
    seq: u64,
    active: Option<Active>,
}

#[derive(Debug)]
struct Active {
    seq: u64,
    alerted: bool,
    reason: String,
}

impl Outage {
    /// A connection attempt failed or the connection dropped. Returns a
    /// sequence number to schedule a check for when a new outage starts.
    pub fn disconnected(&mut self, reason: &str) -> Option<u64> {
        if let Some(active) = &mut self.active {
            active.reason = reason.to_string();
            return None;
        }
        self.seq += 1;
        self.active = Some(Active { seq: self.seq, alerted: false, reason: reason.to_string() });
        Some(self.seq)
    }

    /// The scheduled check fired: the reason to show, if an alert is due now.
    pub fn alert_due(&mut self, seq: u64) -> Option<String> {
        let active = self.active.as_mut().filter(|a| a.seq == seq && !a.alerted)?;
        active.alerted = true;
        Some(active.reason.clone())
    }

    /// Connected again; true if an alert was showing (so say "back again").
    pub fn connected(&mut self) -> bool {
        self.active.take().is_some_and(|a| a.alerted)
    }

    /// After resume from suspend: the waiting time starts over.
    pub fn restart_clock(&mut self) -> Option<u64> {
        let active = self.active.as_mut().filter(|a| !a.alerted)?;
        self.seq += 1;
        active.seq = self.seq;
        Some(self.seq)
    }

    /// Logged out or login expired: stop tracking; true if an alert is showing.
    pub fn cancel(&mut self) -> bool {
        self.connected()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alerts_once_after_the_delay() {
        let mut o = Outage::default();
        let seq = o.disconnected("connection refused").expect("starts the clock");
        assert_eq!(o.disconnected("timed out"), None, "clock keeps running on repeated failures");
        assert_eq!(o.alert_due(seq).as_deref(), Some("timed out"), "latest reason is shown");
        assert_eq!(o.alert_due(seq), None, "only once");
    }

    #[test]
    fn reconnecting_in_time_means_no_alert() {
        let mut o = Outage::default();
        let seq = o.disconnected("x").unwrap();
        assert!(!o.connected(), "nothing was shown, nothing to replace");
        assert_eq!(o.alert_due(seq), None);
    }

    #[test]
    fn connected_after_alert_asks_for_all_clear() {
        let mut o = Outage::default();
        let seq = o.disconnected("x").unwrap();
        o.alert_due(seq).unwrap();
        assert!(o.connected());
        assert!(!o.connected());
    }

    #[test]
    fn wake_restarts_the_clock_before_an_alert() {
        let mut o = Outage::default();
        let first = o.disconnected("x").unwrap();
        let second = o.restart_clock().expect("new deadline after resume");
        assert_eq!(o.alert_due(first), None, "old deadline is void");
        assert!(o.alert_due(second).is_some());
        assert_eq!(o.restart_clock(), None, "already alerted: nothing to restart");
    }

    #[test]
    fn cancel_reports_whether_an_alert_is_showing() {
        let mut o = Outage::default();
        let seq = o.disconnected("x").unwrap();
        assert!(!o.cancel());
        let seq2 = o.disconnected("x").unwrap();
        assert_ne!(seq, seq2);
        o.alert_due(seq2).unwrap();
        assert!(o.cancel());
        assert_eq!(o.alert_due(seq2), None);
    }
}
