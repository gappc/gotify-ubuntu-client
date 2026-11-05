//! Search in the message list (Gotify has no server-side search).

use gotify_core::kuma::{KumaStatus, parse_kuma};
use gotify_core::model::Message;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    pub text: String,
    /// Only messages shown in red: Kuma Down and other high-priority messages.
    pub down_only: bool,
}

impl Search {
    pub fn is_active(&self) -> bool {
        self.down_only || !self.text.trim().is_empty()
    }
}

fn is_problem(msg: &Message) -> bool {
    match parse_kuma(&msg.message) {
        Some(e) => e.status == KumaStatus::Down,
        None => msg.priority >= 8,
    }
}

/// Every word of the search text must appear (ignoring case) in the title,
/// message, monitor name or app name.
pub fn matches(msg: &Message, app_name: Option<&str>, search: &Search) -> bool {
    if search.down_only && !is_problem(msg) {
        return false;
    }
    let haystack = format!("{} {} {}", msg.title.as_deref().unwrap_or(""), msg.message, app_name.unwrap_or(""))
        .to_lowercase();
    search.text.split_whitespace().all(|word| haystack.contains(&word.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn m(title: &str, text: &str, priority: i64) -> Message {
        Message { id: 1, appid: 1, message: text.into(), title: Some(title.into()), priority, date: Utc::now(), extras: None }
    }

    #[test]
    fn empty_search_matches_everything() {
        assert!(matches(&m("Deploy", "ok", 5), Some("CI"), &Search::default()));
    }

    #[test]
    fn text_is_case_insensitive_and_all_words_must_match() {
        let msg = m("Deploy #12 FAILED", "api-3 rolled back", 8);
        let s = |q: &str| Search { text: q.into(), down_only: false };
        assert!(matches(&msg, None, &s("failed")));
        assert!(matches(&msg, None, &s("  ROLLED   api-3 ")));
        assert!(!matches(&msg, None, &s("failed web01")));
    }

    #[test]
    fn monitor_and_app_names_are_searchable() {
        let kuma = m("Uptime-Kuma", "[lynq-prod updates] [🔴 Down] 2 pending", 8);
        let s = |q: &str| Search { text: q.into(), down_only: false };
        assert!(matches(&kuma, None, &s("lynq-prod")));
        assert!(matches(&kuma, Some("Uptime Kuma"), &s("uptime kuma")));
    }

    #[test]
    fn down_only_keeps_problems() {
        let down = Search { text: String::new(), down_only: true };
        assert!(matches(&m("Uptime-Kuma", "[web] [🔴 Down] x", 2), None, &down));
        assert!(!matches(&m("Uptime-Kuma", "[web] [✅ Up] x", 8), None, &down));
        assert!(matches(&m("Deploy failed", "x", 8), None, &down)); // other high-priority = shown red too
        assert!(!matches(&m("Deploy ok", "x", 5), None, &down));
    }

    #[test]
    fn active_only_with_text_or_toggle() {
        assert!(!Search::default().is_active());
        assert!(!Search { text: "   ".into(), down_only: false }.is_active());
        assert!(Search { text: "x".into(), down_only: false }.is_active());
        assert!(Search { text: String::new(), down_only: true }.is_active());
    }
}
