//! Friendly schedules ("every Monday at 09:00") and next-fire computation.
//! Cron is intentionally never exposed.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Frequency {
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    const fn to_chrono(self) -> chrono::Weekday {
        match self {
            Self::Monday => chrono::Weekday::Mon,
            Self::Tuesday => chrono::Weekday::Tue,
            Self::Wednesday => chrono::Weekday::Wed,
            Self::Thursday => chrono::Weekday::Thu,
            Self::Friday => chrono::Weekday::Fri,
            Self::Saturday => chrono::Weekday::Sat,
            Self::Sunday => chrono::Weekday::Sun,
        }
    }
}

fn default_timezone() -> String {
    "UTC".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub every: Frequency,
    /// Local time of day, `HH:MM`.
    pub at: String,
    /// For weekly schedules: the days to run. Defaults to Monday.
    #[serde(default)]
    pub weekdays: Vec<Weekday>,
    /// For monthly schedules: the day of the month, 1 to 31. Months that are
    /// shorter run on their last day.
    #[serde(default)]
    pub day_of_month: Option<u32>,
    /// IANA time zone name such as `Europe/Lisbon`.
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScheduleError {
    #[error("the time of day must be written like 09:30")]
    InvalidTime,
    #[error("the time zone \"{0}\" is not recognised")]
    InvalidTimezone(String),
    #[error("the day of the month must be between 1 and 31")]
    InvalidDayOfMonth,
}

impl Schedule {
    /// Every day at 08:00 UTC; used when a publication alert has no schedule.
    pub fn daily_default() -> Self {
        Self {
            every: Frequency::Day,
            at: "08:00".to_owned(),
            weekdays: Vec::new(),
            day_of_month: None,
            timezone: default_timezone(),
        }
    }

    pub fn validate(&self) -> Result<(), ScheduleError> {
        self.time()?;
        self.zone()?;
        if let Some(day) = self.day_of_month
            && !(1..=31).contains(&day)
        {
            return Err(ScheduleError::InvalidDayOfMonth);
        }
        Ok(())
    }

    fn time(&self) -> Result<NaiveTime, ScheduleError> {
        NaiveTime::parse_from_str(self.at.trim(), "%H:%M").map_err(|_| ScheduleError::InvalidTime)
    }

    fn zone(&self) -> Result<Tz, ScheduleError> {
        self.timezone
            .parse::<Tz>()
            .map_err(|_| ScheduleError::InvalidTimezone(self.timezone.clone()))
    }

    fn runs_on(&self, date: NaiveDate) -> bool {
        match self.every {
            Frequency::Day => true,
            Frequency::Week => {
                if self.weekdays.is_empty() {
                    date.weekday() == chrono::Weekday::Mon
                } else {
                    self.weekdays
                        .iter()
                        .any(|day| day.to_chrono() == date.weekday())
                }
            }
            Frequency::Month => {
                let wanted = self.day_of_month.unwrap_or(1);
                date.day() == wanted.min(days_in_month(date))
            }
        }
    }
}

fn days_in_month(date: NaiveDate) -> u32 {
    let (year, month) = (date.year(), date.month());
    let first_of_next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    first_of_next
        .and_then(|next| next.pred_opt())
        .map_or(31, |last| last.day())
}

fn resolve_local(zone: Tz, naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    zone.from_local_datetime(&naive)
        .earliest()
        .or_else(|| {
            // The wall-clock time does not exist (spring-forward gap): run at
            // the first valid moment after it.
            zone.from_local_datetime(&(naive + Duration::hours(1)))
                .earliest()
        })
        .map(|local| local.with_timezone(&Utc))
}

/// The first moment strictly after `after` at which the schedule fires.
pub fn next_fire_after(
    schedule: &Schedule,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, ScheduleError> {
    schedule.validate()?;
    let time = schedule.time()?;
    let zone = schedule.zone()?;
    let start = after.with_timezone(&zone).date_naive();
    // 13 months covers every monthly/weekly pattern, including day 31.
    for offset in 0..=400 {
        let Some(date) = start.checked_add_signed(Duration::days(offset)) else {
            break;
        };
        if !schedule.runs_on(date) {
            continue;
        }
        if let Some(fire) = resolve_local(zone, date.and_time(time))
            && fire > after
        {
            return Ok(Some(fire));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .expect("valid")
    }

    #[test]
    fn daily_fires_today_then_tomorrow() {
        let schedule = Schedule {
            every: Frequency::Day,
            at: "09:00".to_owned(),
            weekdays: vec![],
            day_of_month: None,
            timezone: "UTC".to_owned(),
        };
        assert_eq!(
            next_fire_after(&schedule, at(2026, 10, 7, 8, 0)).expect("valid"),
            Some(at(2026, 10, 7, 9, 0))
        );
        assert_eq!(
            next_fire_after(&schedule, at(2026, 10, 7, 9, 0)).expect("valid"),
            Some(at(2026, 10, 8, 9, 0))
        );
    }

    #[test]
    fn weekly_picks_the_next_listed_weekday() {
        let schedule = Schedule {
            every: Frequency::Week,
            at: "07:30".to_owned(),
            weekdays: vec![Weekday::Monday, Weekday::Friday],
            day_of_month: None,
            timezone: "UTC".to_owned(),
        };
        // 2026-10-07 is a Wednesday.
        assert_eq!(
            next_fire_after(&schedule, at(2026, 10, 7, 12, 0)).expect("valid"),
            Some(at(2026, 10, 9, 7, 30))
        );
        assert_eq!(
            next_fire_after(&schedule, at(2026, 10, 9, 7, 30)).expect("valid"),
            Some(at(2026, 10, 12, 7, 30))
        );
    }

    #[test]
    fn monthly_clamps_to_the_last_day() {
        let schedule = Schedule {
            every: Frequency::Month,
            at: "00:00".to_owned(),
            weekdays: vec![],
            day_of_month: Some(31),
            timezone: "UTC".to_owned(),
        };
        assert_eq!(
            next_fire_after(&schedule, at(2026, 2, 1, 0, 0)).expect("valid"),
            Some(at(2026, 2, 28, 0, 0))
        );
        assert_eq!(
            next_fire_after(&schedule, at(2026, 2, 28, 0, 0)).expect("valid"),
            Some(at(2026, 3, 31, 0, 0))
        );
    }

    #[test]
    fn time_zones_shift_the_utc_instant_and_follow_dst() {
        let schedule = Schedule {
            every: Frequency::Day,
            at: "09:00".to_owned(),
            weekdays: vec![],
            day_of_month: None,
            timezone: "Europe/Lisbon".to_owned(),
        };
        // Lisbon is UTC+0 in winter and UTC+1 in summer.
        assert_eq!(
            next_fire_after(&schedule, at(2026, 1, 10, 0, 0)).expect("valid"),
            Some(at(2026, 1, 10, 9, 0))
        );
        assert_eq!(
            next_fire_after(&schedule, at(2026, 7, 10, 0, 0)).expect("valid"),
            Some(at(2026, 7, 10, 8, 0))
        );
    }

    #[test]
    fn invalid_schedules_are_reported_in_plain_language() {
        let mut schedule = Schedule::daily_default();
        schedule.at = "25:00".to_owned();
        assert_eq!(schedule.validate(), Err(ScheduleError::InvalidTime));
        let mut schedule = Schedule::daily_default();
        schedule.timezone = "Mars/Base".to_owned();
        assert!(matches!(
            schedule.validate(),
            Err(ScheduleError::InvalidTimezone(_))
        ));
        let mut schedule = Schedule::daily_default();
        schedule.every = Frequency::Month;
        schedule.day_of_month = Some(40);
        assert_eq!(schedule.validate(), Err(ScheduleError::InvalidDayOfMonth));
    }
}
