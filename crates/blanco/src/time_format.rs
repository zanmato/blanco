use chrono::{DateTime, Utc};

/// Format a duration given in microseconds. Sub-millisecond queries
/// would otherwise round to "0ms", so we keep the finest unit and
/// step up to ms / s / m as the value grows.
pub fn format_duration(duration_us: i64) -> String {
    if duration_us < 1_000 {
        return format!("{}µs", duration_us);
    }

    let duration_ms = duration_us / 1_000;
    if duration_ms < 1000 {
        format!("{}ms", duration_ms)
    } else if duration_ms < 60000 {
        let seconds = duration_ms as f64 / 1000.0;
        if seconds < 10.0 {
            format!("{:.1}s", seconds)
        } else {
            format!("{}s", seconds.round() as i64)
        }
    } else {
        let minutes = duration_ms / 60000;
        let seconds = (duration_ms % 60000) / 1000;
        t!(
            "shell.time.minutes_seconds",
            minutes = minutes,
            seconds = seconds
        )
        .into_owned()
    }
}

pub fn format_elapsed(duration: std::time::Duration) -> String {
    format_duration(duration.as_micros() as i64)
}

/// "execution 12ms · fetch 340ms", or the execution time alone for a
/// statement that returned no rows.
pub fn format_timing(timing: &blanco_core::QueryTiming) -> String {
    match timing.fetch {
        Some(fetch) => t!(
            "shell.time.execution_fetch",
            execution = format_elapsed(timing.execution),
            fetch = format_elapsed(fetch)
        )
        .into_owned(),
        None => t!(
            "shell.time.execution",
            execution = format_elapsed(timing.execution)
        )
        .into_owned(),
    }
}

pub fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn format_current_timestamp() -> String {
    format_timestamp(Utc::now())
}

/// Render a Unix timestamp (seconds) as a short, human-friendly relative label
/// such as "just now", "5m ago", "3h ago", or a date for older entries.
pub fn format_relative(timestamp_secs: i64) -> String {
    let now = Utc::now().timestamp();
    let delta = now - timestamp_secs;

    if delta < 0 {
        return t!("shell.time.just_now").into_owned();
    }

    match delta {
        0..=9 => t!("shell.time.just_now").into_owned(),
        10..=59 => t!("shell.time.seconds_ago", count = delta).into_owned(),
        60..=3599 => t!("shell.time.minutes_ago", count = delta / 60).into_owned(),
        3600..=86399 => t!("shell.time.hours_ago", count = delta / 3600).into_owned(),
        86400..=604799 => t!("shell.time.days_ago", count = delta / 86400).into_owned(),
        _ => DateTime::<Utc>::from_timestamp(timestamp_secs, 0)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| t!("shell.time.unknown").into_owned()),
    }
}
