use chrono::{DateTime, Utc};

pub fn format_duration(duration_ms: i64) -> String {
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
        format!("{}m {}s", minutes, seconds)
    }
}

pub fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn format_current_timestamp() -> String {
    format_timestamp(Utc::now())
}
