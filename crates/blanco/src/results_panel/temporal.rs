use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc};

use crate::settings::{DateDisplayFormat, ResultsSettings, TimeZoneDisplay};

/// Which parts a date or time column holds, and so which parts the picker
/// edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemporalShape {
    Date,
    Time,
    Timestamp,
    TimestampWithTimeZone,
}

impl TemporalShape {
    /// The shape of a column declared as `data_type`, across the supported
    /// backends. `None` for types the picker does not edit, such as intervals
    /// and MySQL's `YEAR`.
    pub fn from_data_type(data_type: &str) -> Option<Self> {
        let lower = data_type.trim().to_ascii_lowercase();
        // ClickHouse wraps nullable columns, and precision or a zone argument
        // such as `timestamp(3) with time zone` or `DateTime64(3, 'UTC')` does
        // not change the shape.
        let unwrapped = lower
            .strip_prefix("nullable(")
            .and_then(|inner| inner.strip_suffix(')'))
            .unwrap_or(&lower);
        let mut bare = String::with_capacity(unwrapped.len());
        let mut depth = 0usize;
        for character in unwrapped.chars() {
            match character {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth == 0 => bare.push(character),
                _ => {}
            }
        }
        let bare = bare.split_whitespace().collect::<Vec<_>>().join(" ");

        match bare.as_str() {
            "date" | "date32" => Some(Self::Date),
            "time" | "time without time zone" | "timetz" | "time with time zone" => {
                Some(Self::Time)
            }
            "timestamptz" | "timestamp with time zone" | "datetimeoffset" => {
                Some(Self::TimestampWithTimeZone)
            }
            "timestamp"
            | "timestamp without time zone"
            | "datetime"
            | "datetime2"
            | "smalldatetime"
            | "datetime64" => Some(Self::Timestamp),
            _ => None,
        }
    }

    pub fn has_date(self) -> bool {
        self != Self::Time
    }

    pub fn has_time(self) -> bool {
        self != Self::Date
    }
}

/// A cell value read from the text a driver returns for a date or time column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemporalValue {
    Date(NaiveDate),
    Time(NaiveTime),
    Timestamp(NaiveDateTime),
    TimestampWithTimeZone(DateTime<FixedOffset>),
}

impl TemporalValue {
    /// Read the forms the drivers produce: RFC 3339 from PostgreSQL's
    /// `timestamptz`, `2024-01-31 14:05:09 +01:00` from SQL Server's
    /// `datetimeoffset`, and plain dates, times and timestamps with optional
    /// fractional seconds. Anything else, such as an interval, a year or
    /// MySQL's zero date, is not a value this can show differently.
    pub fn parse(text: &str) -> Option<Self> {
        if let Ok(value) = DateTime::parse_from_rfc3339(text) {
            return Some(Self::TimestampWithTimeZone(value));
        }
        for pattern in ["%Y-%m-%d %H:%M:%S%.f %:z", "%Y-%m-%d %H:%M:%S%.f%#z"] {
            if let Ok(value) = DateTime::parse_from_str(text, pattern) {
                return Some(Self::TimestampWithTimeZone(value));
            }
        }
        for pattern in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
            if let Ok(value) = NaiveDateTime::parse_from_str(text, pattern) {
                return Some(Self::Timestamp(value));
            }
        }
        if let Ok(value) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
            return Some(Self::Date(value));
        }
        NaiveTime::parse_from_str(text, "%H:%M:%S%.f")
            .ok()
            .map(Self::Time)
    }

    /// The current moment in `shape`, to the second. A naive timestamp takes
    /// the local wall clock, as `NOW()` does on a server in the local zone.
    pub fn now(shape: TemporalShape, time_zone: TimeZoneDisplay) -> Self {
        let now = Local::now();
        let now = now.with_nanosecond(0).unwrap_or(now);
        match shape {
            TemporalShape::Date => Self::Date(now.date_naive()),
            TemporalShape::Time => Self::Time(now.time()),
            TemporalShape::Timestamp => Self::Timestamp(now.naive_local()),
            TemporalShape::TimestampWithTimeZone => {
                Self::TimestampWithTimeZone(in_time_zone(now.fixed_offset(), time_zone))
            }
        }
    }

    /// Build a value of `shape` from the parts the picker edits. `offset` only
    /// matters for a timestamp with a time zone.
    pub fn from_parts(
        shape: TemporalShape,
        date: NaiveDate,
        time: NaiveTime,
        offset: FixedOffset,
    ) -> Option<Self> {
        match shape {
            TemporalShape::Date => Some(Self::Date(date)),
            TemporalShape::Time => Some(Self::Time(time)),
            TemporalShape::Timestamp => Some(Self::Timestamp(date.and_time(time))),
            TemporalShape::TimestampWithTimeZone => date
                .and_time(time)
                .and_local_timezone(offset)
                .single()
                .map(Self::TimestampWithTimeZone),
        }
    }

    pub fn shape(&self) -> TemporalShape {
        match self {
            Self::Date(_) => TemporalShape::Date,
            Self::Time(_) => TemporalShape::Time,
            Self::Timestamp(_) => TemporalShape::Timestamp,
            Self::TimestampWithTimeZone(_) => TemporalShape::TimestampWithTimeZone,
        }
    }

    pub fn date(&self) -> Option<NaiveDate> {
        match self {
            Self::Date(date) => Some(*date),
            Self::Time(_) => None,
            Self::Timestamp(value) => Some(value.date()),
            Self::TimestampWithTimeZone(value) => Some(value.date_naive()),
        }
    }

    pub fn time(&self) -> Option<NaiveTime> {
        match self {
            Self::Date(_) => None,
            Self::Time(time) => Some(*time),
            Self::Timestamp(value) => Some(value.time()),
            Self::TimestampWithTimeZone(value) => Some(value.time()),
        }
    }

    pub fn offset(&self) -> Option<FixedOffset> {
        match self {
            Self::TimestampWithTimeZone(value) => Some(*value.offset()),
            _ => None,
        }
    }

    /// The same instant seen from `time_zone`. Values without a zone are
    /// returned as they are.
    pub fn in_time_zone(self, time_zone: TimeZoneDisplay) -> Self {
        match self {
            Self::TimestampWithTimeZone(value) => {
                Self::TimestampWithTimeZone(in_time_zone(value, time_zone))
            }
            other => other,
        }
    }

    /// The text written into the cell editor, in the form the drivers return
    /// so that an edit made with the picker reads like one typed by hand.
    pub fn to_edit_text(&self) -> String {
        match self {
            Self::Date(date) => date.format("%Y-%m-%d").to_string(),
            Self::Time(time) => time.format("%H:%M:%S%.f").to_string(),
            Self::Timestamp(value) => value.format("%Y-%m-%d %H:%M:%S%.f").to_string(),
            Self::TimestampWithTimeZone(value) => {
                value.format("%Y-%m-%dT%H:%M:%S%.f%:z").to_string()
            }
        }
    }

    pub fn display(&self, format: DateDisplayFormat, time_zone: TimeZoneDisplay) -> String {
        let date_pattern = match format {
            DateDisplayFormat::Iso8601 | DateDisplayFormat::Sql => "%Y-%m-%d",
            DateDisplayFormat::DayMonthYear => "%d/%m/%Y",
            DateDisplayFormat::MonthDayYear => "%m/%d/%Y",
        };
        let time_pattern = match format {
            DateDisplayFormat::MonthDayYear => "%-I:%M:%S%.f %p",
            _ => "%H:%M:%S%.f",
        };
        let separator = match format {
            DateDisplayFormat::Iso8601 => "T",
            _ => " ",
        };
        // ISO 8601 and SQL write the offset straight after the time; after a
        // 12-hour time it would run into the AM/PM marker.
        let offset_pattern = match format {
            DateDisplayFormat::Iso8601 | DateDisplayFormat::Sql => "%:z",
            _ => " %:z",
        };
        match self.in_time_zone(time_zone) {
            Self::Date(date) => date.format(date_pattern).to_string(),
            Self::Time(time) => time.format(time_pattern).to_string(),
            Self::Timestamp(value) => value
                .format(&format!("{date_pattern}{separator}{time_pattern}"))
                .to_string(),
            Self::TimestampWithTimeZone(value) => value
                .format(&format!(
                    "{date_pattern}{separator}{time_pattern}{offset_pattern}"
                ))
                .to_string(),
        }
    }
}

fn in_time_zone(value: DateTime<FixedOffset>, time_zone: TimeZoneDisplay) -> DateTime<FixedOffset> {
    match time_zone {
        TimeZoneDisplay::Local => value.with_timezone(&Local).fixed_offset(),
        TimeZoneDisplay::Utc => value.with_timezone(&Utc).fixed_offset(),
    }
}

/// How a date or time cell reads in the grid under the user's settings, or
/// `None` when the text is not a value that can be reformatted and should be
/// shown as it is.
pub fn display_text(value: &str, settings: &ResultsSettings) -> Option<String> {
    TemporalValue::parse(value).map(|value| value.display(settings.date_format, settings.time_zone))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offset(hours: i32) -> FixedOffset {
        FixedOffset::east_opt(hours * 3600).expect("offset in range")
    }

    #[test]
    fn parses_the_forms_drivers_return() {
        let date = NaiveDate::from_ymd_opt(2024, 1, 31).expect("valid date");
        let time = NaiveTime::from_hms_opt(14, 5, 9).expect("valid time");
        let with_micros = NaiveTime::from_hms_micro_opt(14, 5, 9, 123_456).expect("valid time");

        assert_eq!(
            TemporalValue::parse("2024-01-31"),
            Some(TemporalValue::Date(date))
        );
        assert_eq!(
            TemporalValue::parse("14:05:09"),
            Some(TemporalValue::Time(time))
        );
        assert_eq!(
            TemporalValue::parse("14:05:09.123456"),
            Some(TemporalValue::Time(with_micros))
        );
        assert_eq!(
            TemporalValue::parse("2024-01-31 14:05:09"),
            Some(TemporalValue::Timestamp(date.and_time(time)))
        );
        assert_eq!(
            TemporalValue::parse("2024-01-31T14:05:09.123456"),
            Some(TemporalValue::Timestamp(date.and_time(with_micros)))
        );

        let expected = date
            .and_time(time)
            .and_local_timezone(offset(1))
            .single()
            .map(TemporalValue::TimestampWithTimeZone);
        for text in [
            "2024-01-31T14:05:09+01:00",
            "2024-01-31 14:05:09 +01:00",
            "2024-01-31 14:05:09+01",
        ] {
            assert_eq!(TemporalValue::parse(text), expected, "parsing {text}");
        }
    }

    #[test]
    fn leaves_values_it_cannot_read() {
        for text in [
            "1 day 02:00:00",
            "2024",
            "0000-00-00",
            "infinity",
            "",
            "now()",
        ] {
            assert_eq!(TemporalValue::parse(text), None, "parsing {text}");
        }
    }

    #[test]
    fn displays_each_format() {
        let value = TemporalValue::parse("2024-01-31 14:05:09").expect("timestamp");
        let display = |format| value.display(format, TimeZoneDisplay::Utc);
        assert_eq!(display(DateDisplayFormat::Iso8601), "2024-01-31T14:05:09");
        assert_eq!(display(DateDisplayFormat::Sql), "2024-01-31 14:05:09");
        assert_eq!(
            display(DateDisplayFormat::DayMonthYear),
            "31/01/2024 14:05:09"
        );
        assert_eq!(
            display(DateDisplayFormat::MonthDayYear),
            "01/31/2024 2:05:09 PM"
        );

        let date = TemporalValue::parse("2024-01-31").expect("date");
        assert_eq!(
            date.display(DateDisplayFormat::DayMonthYear, TimeZoneDisplay::Utc),
            "31/01/2024"
        );
        let time = TemporalValue::parse("09:05:09.250").expect("time");
        assert_eq!(
            time.display(DateDisplayFormat::MonthDayYear, TimeZoneDisplay::Utc),
            "9:05:09.250 AM"
        );
    }

    #[test]
    fn shows_zoned_timestamps_in_utc_or_local_time() {
        let value = TemporalValue::parse("2024-01-31T14:05:09+02:00").expect("timestamptz");
        assert_eq!(
            value.display(DateDisplayFormat::Iso8601, TimeZoneDisplay::Utc),
            "2024-01-31T12:05:09+00:00"
        );
        assert_eq!(
            value.display(DateDisplayFormat::DayMonthYear, TimeZoneDisplay::Utc),
            "31/01/2024 12:05:09 +00:00"
        );

        let local = value.in_time_zone(TimeZoneDisplay::Local).to_edit_text();
        let expected = DateTime::parse_from_rfc3339("2024-01-31T14:05:09+02:00")
            .map(|instant| {
                instant
                    .with_timezone(&Local)
                    .format("%Y-%m-%dT%H:%M:%S%:z")
                    .to_string()
            })
            .expect("valid timestamp");
        assert_eq!(local, expected);

        let naive = TemporalValue::parse("2024-01-31 14:05:09").expect("timestamp");
        assert_eq!(
            naive.in_time_zone(TimeZoneDisplay::Utc),
            naive,
            "a timestamp without a zone is never shifted"
        );
    }

    #[test]
    fn edit_text_matches_what_drivers_return() {
        for text in [
            "2024-01-31",
            "14:05:09",
            "14:05:09.123456",
            "2024-01-31 14:05:09",
            "2024-01-31 14:05:09.5",
            "2024-01-31T14:05:09+01:00",
        ] {
            let value = TemporalValue::parse(text).expect("parsable");
            let round_trip = TemporalValue::parse(&value.to_edit_text());
            assert_eq!(round_trip, Some(value), "round trip of {text}");
        }
        assert_eq!(
            TemporalValue::parse("2024-01-31 14:05:09")
                .expect("timestamp")
                .to_edit_text(),
            "2024-01-31 14:05:09"
        );
    }

    #[test]
    fn builds_values_from_picker_parts() {
        let date = NaiveDate::from_ymd_opt(2024, 3, 1).expect("valid date");
        let time = NaiveTime::from_hms_opt(8, 0, 0).expect("valid time");
        let build = |shape| TemporalValue::from_parts(shape, date, time, offset(-5));
        assert_eq!(
            build(TemporalShape::Date).map(|value| value.to_edit_text()),
            Some("2024-03-01".to_string())
        );
        assert_eq!(
            build(TemporalShape::Time).map(|value| value.to_edit_text()),
            Some("08:00:00".to_string())
        );
        assert_eq!(
            build(TemporalShape::Timestamp).map(|value| value.to_edit_text()),
            Some("2024-03-01 08:00:00".to_string())
        );
        assert_eq!(
            build(TemporalShape::TimestampWithTimeZone).map(|value| value.to_edit_text()),
            Some("2024-03-01T08:00:00-05:00".to_string())
        );
    }

    #[test]
    fn now_has_the_requested_shape_and_whole_seconds() {
        for shape in [
            TemporalShape::Date,
            TemporalShape::Time,
            TemporalShape::Timestamp,
            TemporalShape::TimestampWithTimeZone,
        ] {
            let value = TemporalValue::now(shape, TimeZoneDisplay::Utc);
            assert_eq!(value.shape(), shape);
            assert!(value.time().is_none_or(|time| time.nanosecond() == 0));
        }
        assert_eq!(
            TemporalValue::now(TemporalShape::TimestampWithTimeZone, TimeZoneDisplay::Utc).offset(),
            Some(offset(0))
        );
    }

    #[test]
    fn reads_shapes_from_column_types() {
        let cases = [
            ("date", Some(TemporalShape::Date)),
            ("Date32", Some(TemporalShape::Date)),
            ("Nullable(Date)", Some(TemporalShape::Date)),
            ("time without time zone", Some(TemporalShape::Time)),
            ("time(3)", Some(TemporalShape::Time)),
            ("timetz", Some(TemporalShape::Time)),
            ("timestamp", Some(TemporalShape::Timestamp)),
            (
                "timestamp(3) without time zone",
                Some(TemporalShape::Timestamp),
            ),
            ("DATETIME", Some(TemporalShape::Timestamp)),
            ("datetime2(7)", Some(TemporalShape::Timestamp)),
            ("DateTime64(3, 'UTC')", Some(TemporalShape::Timestamp)),
            ("timestamptz", Some(TemporalShape::TimestampWithTimeZone)),
            (
                "timestamp(6) with time zone",
                Some(TemporalShape::TimestampWithTimeZone),
            ),
            ("datetimeoffset", Some(TemporalShape::TimestampWithTimeZone)),
            ("interval", None),
            ("year", None),
            ("text", None),
        ];
        for (data_type, expected) in cases {
            assert_eq!(
                TemporalShape::from_data_type(data_type),
                expected,
                "shape of {data_type}"
            );
        }
    }

    #[test]
    fn setting_keys_round_trip() {
        for format in DateDisplayFormat::ALL {
            assert_eq!(DateDisplayFormat::from_key(format.key()), Some(format));
        }
        for zone in TimeZoneDisplay::ALL {
            assert_eq!(TimeZoneDisplay::from_key(zone.key()), Some(zone));
        }
        assert_eq!(DateDisplayFormat::default(), DateDisplayFormat::Iso8601);
    }
}
