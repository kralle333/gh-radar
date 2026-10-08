use chrono::{DateTime, Duration, Utc};

/// Compact relative age: `now`, `42s`, `5m`, `3h`, `2d`, `4mo`, `1y`.
pub fn ago(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let s = (now - t).num_seconds().max(0);
    match s {
        0..5 => "now".into(),
        5..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        86400..2_592_000 => format!("{}d", s / 86400),
        2_592_000..31_536_000 => format!("{}mo", s / 2_592_000),
        _ => format!("{}y", s / 31_536_000),
    }
}

/// Compact duration: `45s`, `3m07s`, `1h12m`.
pub fn duration(d: Duration) -> String {
    let s = d.num_seconds().max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_ages() {
        let now = Utc::now();
        assert_eq!(ago(now, now), "now");
        assert_eq!(ago(now - Duration::seconds(42), now), "42s");
        assert_eq!(ago(now - Duration::minutes(5), now), "5m");
        assert_eq!(ago(now - Duration::hours(3), now), "3h");
        assert_eq!(ago(now - Duration::days(2), now), "2d");
        assert_eq!(ago(now - Duration::days(65), now), "2mo");
        // Clock skew must not produce negative ages.
        assert_eq!(ago(now + Duration::seconds(30), now), "now");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration(Duration::seconds(45)), "45s");
        assert_eq!(duration(Duration::seconds(187)), "3m07s");
        assert_eq!(duration(Duration::seconds(4320)), "1h12m");
    }

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("a longer string", 6), "a lon…");
    }
}
