//! Local calendar ranges converted to UTC instants.

use chrono::{DateTime, Datelike, Days, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use thiserror::Error;

/// Daylight-saving gaps are at most a few hours long.
const MAX_GAP_MINUTES: u32 = 3 * 60;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RangeError {
    #[error("invalid date {0:?}, expected YYYY-MM-DD")]
    Date(String),
    #[error("--to must not be before --from")]
    Order,
    #[error("no local time on {0} exists in this time zone")]
    Midnight(NaiveDate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Span {
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub label: String,
}

pub fn span_range<Tz: TimeZone>(
    span: Span,
    today: NaiveDate,
    tz: &Tz,
) -> Result<Range, RangeError> {
    local_days(first_day(span, today), today, tz)
}

/// The first local date of a span that ends today.
pub fn first_day(span: Span, today: NaiveDate) -> NaiveDate {
    match span {
        Span::Day => today,
        Span::Week => today - Days::new(6),
        Span::Month => today.with_day(1).expect("day 1 exists in every month"),
    }
}

pub fn dates_range<Tz: TimeZone>(
    from: &str,
    to: Option<&str>,
    today: NaiveDate,
    tz: &Tz,
) -> Result<Range, RangeError> {
    let first = parse_date(from)?;
    let last = to.map(parse_date).transpose()?.unwrap_or(today);
    if last < first {
        return Err(RangeError::Order);
    }
    local_days(first, last, tz)
}

fn local_days<Tz: TimeZone>(
    first: NaiveDate,
    last: NaiveDate,
    tz: &Tz,
) -> Result<Range, RangeError> {
    let label = if first == last {
        first.to_string()
    } else {
        format!("{first} – {last}")
    };
    Ok(Range {
        from: day_start(first, tz)?,
        to: day_start(last + Days::new(1), tz)?,
        label,
    })
}

/// The start of a local day: midnight, or the first instant after a daylight-saving gap at midnight.
pub(crate) fn day_start<Tz: TimeZone>(
    date: NaiveDate,
    tz: &Tz,
) -> Result<DateTime<Utc>, RangeError> {
    (0..=MAX_GAP_MINUTES)
        .step_by(15)
        .filter_map(|minute| date.and_hms_opt(minute / 60, minute % 60, 0))
        .find_map(|local| local.and_local_timezone(tz.clone()).earliest())
        .map(|local| local.with_timezone(&Utc))
        .ok_or(RangeError::Midnight(date))
}

fn parse_date(text: &str) -> Result<NaiveDate, RangeError> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| RangeError::Date(text.to_owned()))
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone, Utc};

    use super::*;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(2 * 3600).unwrap()
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 18).unwrap()
    }

    #[test]
    fn day_is_local_midnight_to_midnight() {
        let range = span_range(Span::Day, today(), &tz()).unwrap();
        assert_eq!(
            range.from,
            Utc.with_ymd_and_hms(2026, 3, 17, 22, 0, 0).unwrap()
        );
        assert_eq!(
            range.to,
            Utc.with_ymd_and_hms(2026, 3, 18, 22, 0, 0).unwrap()
        );
        assert_eq!(range.label, "2026-03-18");
    }

    #[test]
    fn week_and_month() {
        let week = span_range(Span::Week, today(), &tz()).unwrap();
        assert_eq!(week.label, "2026-03-12 – 2026-03-18");
        let month = span_range(Span::Month, today(), &tz()).unwrap();
        assert_eq!(month.label, "2026-03-01 – 2026-03-18");
    }

    #[test]
    fn explicit_dates_are_inclusive() {
        let range = dates_range("2026-03-01", Some("2026-03-02"), today(), &Utc).unwrap();
        assert_eq!(range.to - range.from, chrono::TimeDelta::days(2));
        assert!(matches!(
            dates_range("2026-03-05", Some("2026-03-01"), today(), &Utc),
            Err(RangeError::Order)
        ));
        assert!(matches!(
            dates_range("03/01/2026", None, today(), &Utc),
            Err(RangeError::Date(_))
        ));
    }

    #[test]
    fn a_day_whose_midnight_is_skipped_starts_at_the_first_valid_instant() {
        let santiago = chrono_tz::America::Santiago;
        let day = NaiveDate::from_ymd_opt(2026, 9, 6).unwrap();
        let range = span_range(Span::Day, day, &santiago).unwrap();
        assert_eq!(
            range.from,
            Utc.with_ymd_and_hms(2026, 9, 6, 4, 0, 0).unwrap()
        );
        assert_eq!(range.to - range.from, chrono::TimeDelta::hours(23));
    }
}
