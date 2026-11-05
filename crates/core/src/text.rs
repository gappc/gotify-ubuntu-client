//! Small text helpers: links, markup escaping, time labels.

use chrono::{DateTime, Datelike, Local, TimeZone, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment<'a> {
    Text(&'a str),
    Link(&'a str),
}

/// Byte range of the next http(s) URL at or after `from`.
fn next_url(text: &str, from: usize) -> Option<(usize, usize)> {
    let rest = &text[from..];
    let start = [rest.find("https://"), rest.find("http://")].into_iter().flatten().min()? + from;
    let mut end = text[start..].find(char::is_whitespace).map_or(text.len(), |e| start + e);
    // Trailing punctuation usually belongs to the sentence, not the URL.
    while end > start {
        let c = text[..end].chars().next_back().unwrap();
        let unbalanced_close = matches!(c, ')' | ']')
            && text[start..end].matches(if c == ')' { '(' } else { '[' }).count()
                < text[start..end].matches(c).count();
        if matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '>') || unbalanced_close {
            end -= c.len_utf8();
        } else {
            break;
        }
    }
    let scheme_len = if text[start..].starts_with("https://") { 8 } else { 7 };
    (end > start + scheme_len).then_some((start, end))
}

pub fn first_url(text: &str) -> Option<&str> {
    next_url(text, 0).map(|(s, e)| &text[s..e])
}

pub fn segments(text: &str) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some((s, e)) = next_url(text, pos) {
        if s > pos {
            out.push(Segment::Text(&text[pos..s]));
        }
        out.push(Segment::Link(&text[s..e]));
        pos = e;
    }
    if pos < text.len() {
        out.push(Segment::Text(&text[pos..]));
    }
    out
}

pub fn escape_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// "just now", "5 min ago", "3 h ago", "yesterday", "12 Oct", "12 Oct 2025".
pub fn relative_time(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    relative_time_in(t, now, &Local)
}

pub fn relative_time_in<Tz: TimeZone>(t: DateTime<Utc>, now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let secs = (now - t).num_seconds();
    if secs < 60 {
        return "just now".into();
    }
    if secs < 3600 {
        return format!("{} min ago", secs / 60);
    }
    if secs < 12 * 3600 {
        return format!("{} h ago", secs / 3600);
    }
    let (t, now) = (t.with_timezone(tz), now.with_timezone(tz));
    if t.date_naive() == now.date_naive() {
        format!("{} h ago", secs / 3600)
    } else if now.date_naive().pred_opt() == Some(t.date_naive()) {
        "yesterday".into()
    } else if t.year() == now.year() {
        t.format("%-d %b").to_string()
    } else {
        t.format("%-d %b %Y").to_string()
    }
}

/// For "down since …": "14:32" today, "Mon 14:32" this week, else the date.
pub fn since_label<Tz: TimeZone>(t: DateTime<Tz>, now: DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let days = (now.date_naive() - t.date_naive()).num_days();
    if days == 0 {
        t.format("%H:%M").to_string()
    } else if (1..=6).contains(&days) {
        t.format("%a %H:%M").to_string()
    } else {
        t.format("%Y-%m-%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn first_url_strips_trailing_punctuation() {
        assert_eq!(first_url("see https://ci.example.com/job/1."), Some("https://ci.example.com/job/1"));
        assert_eq!(first_url("(http://a.b/c)"), Some("http://a.b/c"));
        assert_eq!(first_url("no link"), None);
    }

    #[test]
    fn segments_split() {
        assert_eq!(
            segments("a https://x.y b"),
            [Segment::Text("a "), Segment::Link("https://x.y"), Segment::Text(" b")]
        );
        assert_eq!(segments("plain"), [Segment::Text("plain")]);
    }

    #[test]
    fn escape() {
        assert_eq!(escape_markup("<b>&"), "&lt;b&gt;&amp;");
        assert_eq!(escape_markup("\"'"), "&quot;&apos;");
    }

    #[test]
    fn relative_time_labels() {
        let now = Utc.with_ymd_and_hms(2026, 11, 5, 15, 0, 0).unwrap();
        let r = |t| relative_time_in(t, now, &Utc);
        assert_eq!(r(now - chrono::Duration::seconds(30)), "just now");
        assert_eq!(r(now - chrono::Duration::minutes(5)), "5 min ago");
        assert_eq!(r(now - chrono::Duration::hours(3)), "3 h ago");
        assert_eq!(r(Utc.with_ymd_and_hms(2026, 11, 4, 1, 0, 0).unwrap()), "yesterday");
        assert_eq!(r(Utc.with_ymd_and_hms(2026, 10, 12, 9, 0, 0).unwrap()), "12 Oct");
        assert_eq!(r(Utc.with_ymd_and_hms(2025, 10, 12, 9, 0, 0).unwrap()), "12 Oct 2025");
    }

    #[test]
    fn since_label_formats() {
        let now = Utc.with_ymd_and_hms(2026, 11, 5, 18, 0, 0).unwrap(); // a Thursday
        assert_eq!(since_label(Utc.with_ymd_and_hms(2026, 11, 5, 14, 32, 0).unwrap(), now), "14:32");
        assert_eq!(since_label(Utc.with_ymd_and_hms(2026, 11, 2, 14, 32, 0).unwrap(), now), "Mon 14:32");
        assert_eq!(since_label(Utc.with_ymd_and_hms(2026, 10, 1, 14, 32, 0).unwrap(), now), "2026-10-01");
    }
}
