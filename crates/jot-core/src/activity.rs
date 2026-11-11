//! Daily statistics over the local calendar, independent of the UI language.

use std::collections::BTreeMap;
use time::{Date, Duration, OffsetDateTime};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Totals {
    pub words: u64,
    pub dictations: u64,
    pub timed_words: u64,
    pub timed_dictations: u64,
    pub audio_seconds: f64,
    pub measured_pipeline: u64,
    pub pipeline_seconds: f64,
    pub failed: u64,
    pub cancelled: u64,
    pub queued: u64,
}

impl Totals {
    pub fn wpm(self) -> Option<f64> {
        (self.audio_seconds > 0.0).then(|| 60.0 * self.timed_words as f64 / self.audio_seconds)
    }

    pub fn estimated_seconds_saved(self, typing_wpm: u16) -> Option<f64> {
        (self.timed_dictations > 0 && typing_wpm > 0).then(|| {
            60.0 * self.timed_words as f64 / f64::from(typing_wpm)
                - self.audio_seconds
                - self.pipeline_seconds
        })
    }
}

impl std::ops::AddAssign for Totals {
    fn add_assign(&mut self, other: Self) {
        self.words += other.words;
        self.dictations += other.dictations;
        self.timed_words += other.timed_words;
        self.timed_dictations += other.timed_dictations;
        self.audio_seconds += other.audio_seconds;
        self.measured_pipeline += other.measured_pipeline;
        self.pipeline_seconds += other.pipeline_seconds;
        self.failed += other.failed;
        self.cancelled += other.cancelled;
        self.queued += other.queued;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Week,
    Month,
    Quarter,
    Year,
    All,
}

impl Period {
    pub const ALL: [Self; 5] = [
        Self::Week,
        Self::Month,
        Self::Quarter,
        Self::Year,
        Self::All,
    ];

    pub fn span(self, today: Date, first: Option<Date>) -> Span {
        let start = match self {
            Self::Week => today.saturating_sub(Duration::days(6)),
            Self::Month => today.saturating_sub(Duration::days(29)),
            Self::Quarter => today.saturating_sub(Duration::days(89)),
            Self::Year => {
                let (year, month, day) = today.to_calendar_date();
                // A leap day has no anniversary in the preceding year.
                Date::from_calendar_date(year - 1, month, day)
                    .or_else(|_| Date::from_calendar_date(year - 1, month, 28))
                    .ok()
                    .and_then(Date::next_day)
                    .unwrap_or(Date::MIN)
            }
            Self::All => first.unwrap_or(today),
        };
        Span { start, end: today }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: Date,
    pub end: Date,
}

impl Span {
    pub fn days(self) -> i64 {
        (self.end - self.start).whole_days() + 1
    }

    pub fn previous(self) -> Option<Self> {
        Some(Self {
            start: self.start.checked_sub(Duration::days(self.days()))?,
            end: self.start.previous_day()?,
        })
    }

    pub fn buckets(self) -> Vec<Self> {
        let days = self.days();
        let mut buckets = Vec::new();
        let mut start = self.start;
        loop {
            let end = match days {
                1..=30 => start,
                31..=90 => start.saturating_add(Duration::days(6)),
                91..=730 => Date::from_calendar_date(
                    start.year(),
                    start.month(),
                    start.month().length(start.year()),
                )
                .expect("last day of a valid month"),
                _ => start.saturating_add(Duration::days((days - 1) / 24)),
            }
            .min(self.end);
            buckets.push(Self { start, end });
            if end == self.end {
                return buckets;
            }
            start = end.next_day().expect("end precedes the last day");
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Streaks {
    pub active_days: u64,
    pub current: u64,
    pub longest: u64,
}

#[derive(Debug)]
pub struct Activity {
    pub today: Date,
    pub days: BTreeMap<Date, Totals>,
}

impl Activity {
    pub fn total(&self, span: Span) -> Totals {
        let mut total = Totals::default();
        for (_, day) in self.days.range(span.start..=span.end) {
            total += *day;
        }
        total
    }

    pub fn streaks(&self) -> Streaks {
        let mut result = Streaks::default();
        let mut previous = None;
        let mut run = 0;
        for (&date, day) in self.days.range(..=self.today) {
            if day.dictations == 0 {
                continue;
            }
            result.active_days += 1;
            run = if date.previous_day() == previous {
                run + 1
            } else {
                1
            };
            result.longest = result.longest.max(run);
            previous = Some(date);
        }
        if previous.is_some()
            && (previous == Some(self.today) || previous == self.today.previous_day())
        {
            result.current = run;
        }
        result
    }
}

#[cfg(windows)]
pub(crate) struct LocalCalendar(windows::Win32::System::Time::DYNAMIC_TIME_ZONE_INFORMATION);

#[cfg(windows)]
impl LocalCalendar {
    pub(crate) fn current() -> anyhow::Result<Self> {
        use windows::Win32::System::Time::GetDynamicTimeZoneInformation;
        let mut zone = Default::default();
        if unsafe { GetDynamicTimeZoneInformation(&mut zone) } == u32::MAX {
            return Err(windows::core::Error::from_win32().into());
        }
        Ok(Self(zone))
    }

    pub(crate) fn date(&self, timestamp: OffsetDateTime) -> anyhow::Result<Date> {
        Ok(self.datetime(timestamp)?.date())
    }

    fn datetime(&self, timestamp: OffsetDateTime) -> anyhow::Result<OffsetDateTime> {
        use windows::Win32::{
            Foundation::SYSTEMTIME, System::Time::SystemTimeToTzSpecificLocalTimeEx,
        };
        let utc = timestamp.to_offset(time::UtcOffset::UTC);
        let universal = SYSTEMTIME {
            wYear: utc.year().try_into()?,
            wMonth: u8::from(utc.month()).into(),
            wDay: utc.day().into(),
            wHour: utc.hour().into(),
            wMinute: utc.minute().into(),
            wSecond: utc.second().into(),
            ..Default::default()
        };
        let mut local = SYSTEMTIME::default();
        unsafe { SystemTimeToTzSpecificLocalTimeEx(Some(&self.0), &universal, &mut local)? };
        let wall = Date::from_calendar_date(
            local.wYear.into(),
            u8::try_from(local.wMonth)?.try_into()?,
            local.wDay.try_into()?,
        )?
        .with_hms(
            local.wHour.try_into()?,
            local.wMinute.try_into()?,
            local.wSecond.try_into()?,
        )?;
        let seconds = wall.assume_utc().unix_timestamp() - utc.unix_timestamp();
        Ok(timestamp.to_offset(time::UtcOffset::from_whole_seconds(seconds.try_into()?)?))
    }
}

#[cfg(not(windows))]
pub(crate) struct LocalCalendar;

#[cfg(not(windows))]
impl LocalCalendar {
    pub(crate) fn current() -> anyhow::Result<Self> {
        Ok(Self)
    }

    pub(crate) fn date(&self, timestamp: OffsetDateTime) -> anyhow::Result<Date> {
        Ok(timestamp
            .to_offset(time::UtcOffset::local_offset_at(timestamp)?)
            .date())
    }
}

/// Converts an instant using the operating system's time zone rules at that date.
pub fn local_datetime(at: OffsetDateTime) -> anyhow::Result<OffsetDateTime> {
    #[cfg(windows)]
    {
        LocalCalendar::current()?.datetime(at)
    }
    #[cfg(not(windows))]
    {
        Ok(at.to_offset(time::UtcOffset::local_offset_at(at)?))
    }
}

/// Delay until the next local date change, computed from actual UTC instants.
pub fn until_next_local_day(now: OffsetDateTime) -> anyhow::Result<std::time::Duration> {
    let calendar = LocalCalendar::current()?;
    until_next_day(now, &calendar)
}

fn until_next_day(
    now: OffsetDateTime,
    calendar: &LocalCalendar,
) -> anyhow::Result<std::time::Duration> {
    let today = calendar.date(now)?;
    let mut low = now.unix_timestamp();
    let mut high = low + 1800;
    while calendar.date(OffsetDateTime::from_unix_timestamp(high)?)? == today {
        low = high;
        high += 1800;
    }
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if calendar.date(OffsetDateTime::from_unix_timestamp(middle)?)? == today {
            low = middle;
        } else {
            high = middle;
        }
    }
    Ok((OffsetDateTime::from_unix_timestamp(high)? - now).unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{date, datetime};

    #[test]
    fn periods_include_today_and_compare_disjoint_equal_lengths() {
        let today = date!(2024 - 02 - 29);
        for (period, length) in [
            (Period::Week, 7),
            (Period::Month, 30),
            (Period::Quarter, 90),
            (Period::Year, 366),
        ] {
            let span = period.span(today, None);
            let previous = span.previous().unwrap();
            assert_eq!(span.end, today);
            assert_eq!(span.days(), length);
            assert_eq!(previous.days(), length);
            assert_eq!(previous.end.next_day(), Some(span.start));
        }
        let year = Period::Year.span(today, None);
        assert_eq!(year.start, date!(2023 - 03 - 01));
        let buckets = year.buckets();
        assert_eq!(buckets.last().unwrap().end, today);
        assert_eq!(buckets.iter().map(|bucket| bucket.days()).sum::<i64>(), 366);
        assert_eq!(
            Period::All.span(today, Some(date!(2023 - 12 - 31))).start,
            date!(2023 - 12 - 31)
        );
        assert_eq!(Period::All.span(today, None).days(), 1);
    }

    #[test]
    fn streaks_skip_failures_and_break_only_after_a_full_inactive_day() {
        let success = Totals {
            words: 1,
            dictations: 1,
            ..Totals::default()
        };
        let mut activity = Activity {
            today: date!(2024 - 03 - 02),
            days: BTreeMap::from([
                (date!(2024 - 02 - 28), success),
                (date!(2024 - 02 - 29), success),
                (date!(2024 - 03 - 01), success),
                (
                    date!(2024 - 03 - 02),
                    Totals {
                        failed: 2,
                        ..Totals::default()
                    },
                ),
            ]),
        };
        assert_eq!(
            activity.streaks(),
            Streaks {
                active_days: 3,
                current: 3,
                longest: 3
            }
        );
        activity.today = date!(2024 - 03 - 03);
        assert_eq!(activity.streaks().current, 0);
        assert_eq!(activity.streaks().longest, 3);
        assert_eq!(
            activity.total(Span {
                start: activity.today,
                end: activity.today
            }),
            Totals::default()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_converts_local_midnight_with_the_offset_at_each_instant() {
        use windows::Win32::{Foundation::SYSTEMTIME, System::Time::DYNAMIC_TIME_ZONE_INFORMATION};
        let mut zone = DYNAMIC_TIME_ZONE_INFORMATION {
            Bias: -60,
            DaylightBias: -60,
            DaylightDate: SYSTEMTIME {
                wMonth: 3,
                wDay: 5,
                wHour: 2,
                ..Default::default()
            },
            StandardDate: SYSTEMTIME {
                wMonth: 10,
                wDay: 5,
                wHour: 3,
                ..Default::default()
            },
            ..Default::default()
        };
        for (unit, key) in zone
            .TimeZoneKeyName
            .iter_mut()
            .zip("Romance Standard Time".encode_utf16())
        {
            *unit = key;
        }
        let calendar = LocalCalendar(zone);
        for (utc, expected) in [
            (datetime!(2023-12-31 23:00 UTC), date!(2024 - 01 - 01)),
            (datetime!(2024-02-28 23:00 UTC), date!(2024 - 02 - 29)),
            (datetime!(2024-03-30 22:59:59 UTC), date!(2024 - 03 - 30)),
            (datetime!(2024-03-30 23:00 UTC), date!(2024 - 03 - 31)),
            (datetime!(2024-03-31 21:59:59 UTC), date!(2024 - 03 - 31)),
            (datetime!(2024-03-31 22:00 UTC), date!(2024 - 04 - 01)),
            (datetime!(2024-10-26 22:00 UTC), date!(2024 - 10 - 27)),
            (datetime!(2024-10-27 22:59:59 UTC), date!(2024 - 10 - 27)),
            (datetime!(2024-10-27 23:00 UTC), date!(2024 - 10 - 28)),
        ] {
            assert_eq!(calendar.date(utc).unwrap(), expected);
        }
        assert_eq!(
            until_next_day(datetime!(2024-03-30 23:00 UTC), &calendar).unwrap(),
            std::time::Duration::from_secs(23 * 3600)
        );
        assert_eq!(
            until_next_day(datetime!(2024-10-26 22:00 UTC), &calendar).unwrap(),
            std::time::Duration::from_secs(25 * 3600)
        );
    }
}
