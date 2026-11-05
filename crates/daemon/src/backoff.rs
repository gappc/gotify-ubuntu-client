//! Reconnect delays: 1 s, doubling, capped at 60 s.

use std::time::Duration;

use gotify_core::consts::{BACKOFF_MAX, BACKOFF_START};

#[derive(Debug, Default)]
pub struct Backoff {
    next: Option<Duration>,
}

impl Backoff {
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next.unwrap_or(BACKOFF_START);
        self.next = Some((delay * 2).min(BACKOFF_MAX));
        delay
    }

    pub fn reset(&mut self) {
        self.next = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn backoff_doubles_and_caps() {
        let mut b = Backoff::default();
        let secs: Vec<u64> = (0..8).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 32, 60, 60]);
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }
}
