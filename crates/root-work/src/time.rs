//! Time helpers for durable work state.

use chrono::{DateTime, SecondsFormat, Utc};

/// Canonical UTC timestamp format stored in the work database.
pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Human-friendly age for a stored RFC 3339 timestamp.
pub fn humanize_age(timestamp: &str) -> String {
    let Ok(parsed) = DateTime::parse_from_rfc3339(timestamp) else {
        return timestamp.to_string();
    };
    let now = Utc::now();
    let delta = now.signed_duration_since(parsed.with_timezone(&Utc));
    let seconds = delta.num_seconds().max(0);
    match seconds {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{} seconds ago", seconds),
        60..=119 => "1 minute ago".to_string(),
        120..=3599 => format!("{} minutes ago", seconds / 60),
        3600..=7199 => "1 hour ago".to_string(),
        7200..=86399 => format!("{} hours ago", seconds / 3600),
        86400..=172799 => "1 day ago".to_string(),
        _ => format!("{} days ago", seconds / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_recent_timestamps() {
        assert_eq!(humanize_age(&now_rfc3339()), "just now");
        assert_eq!(humanize_age("not-a-date"), "not-a-date");
    }
}
