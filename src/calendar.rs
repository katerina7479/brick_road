use std::collections::HashSet;

use chrono::{Datelike, Duration, NaiveDate};

use crate::model::{CalendarConfig, Day};

/// Returns true if `date` is a working day under `config`.
///
/// A day is non-working if:
/// - Its ISO weekday number (Mon=1 … Sun=7) exceeds `working_days_per_week`, or
/// - It appears in `config.non_working_dates`.
fn is_working_day(date: NaiveDate, working_days_per_week: u8, non_working: &HashSet<NaiveDate>) -> bool {
    if non_working.contains(&date) {
        return false;
    }
    date.weekday().number_from_monday() <= working_days_per_week as u32
}

/// Converts an abstract timeline day number to a real calendar date.
///
/// Day 0 maps to `config.start_date`. Positive days advance forward through
/// working days; negative days go backward. Fractional days are truncated.
///
/// Non-working days (weekends beyond `working_days_per_week` and dates in
/// `non_working_dates`) are skipped during the count.
pub fn day_to_date(day: Day, config: &CalendarConfig) -> NaiveDate {
    let working_days = day as i64;
    let non_working: HashSet<NaiveDate> = config.non_working_dates.iter().copied().collect();

    if working_days == 0 {
        return config.start_date;
    }

    let step = if working_days > 0 { 1i64 } else { -1i64 };
    let mut remaining = working_days.abs();
    let mut current = config.start_date;

    while remaining > 0 {
        current = current
            .checked_add_signed(Duration::days(step))
            .unwrap_or(current);
        if is_working_day(current, config.working_days_per_week, &non_working) {
            remaining -= 1;
        }
    }
    current
}

/// Converts a real calendar date to an abstract timeline day number.
///
/// Returns the number of working days in the half-open interval
/// `(config.start_date, date]` (positive) or `(date, config.start_date]`
/// negated (negative). `config.start_date` itself returns 0.
///
/// This is the inverse of `day_to_date` for integer working-day values.
pub fn date_to_day(date: NaiveDate, config: &CalendarConfig) -> i32 {
    if date == config.start_date {
        return 0;
    }

    let non_working: HashSet<NaiveDate> = config.non_working_dates.iter().copied().collect();

    let (start, end, sign) = if date > config.start_date {
        (config.start_date, date, 1i32)
    } else {
        (date, config.start_date, -1i32)
    };

    let mut count = 0i32;
    let mut current = start;
    while current < end {
        current = current
            .checked_add_signed(Duration::days(1))
            .unwrap_or(current);
        if is_working_day(current, config.working_days_per_week, &non_working) {
            count += 1;
        }
    }
    count * sign
}

/// Converts effort in working days to a calendar duration in calendar days.
///
/// Returns the number of calendar days from `start_date` to complete
/// `effort_days` working days of work.
pub fn effort_to_calendar_days(effort_days: Day, start_date: NaiveDate, config: &CalendarConfig) -> i64 {
    let finish = day_to_date(effort_days, &CalendarConfig {
        start_date,
        ..config.clone()
    });
    (finish - start_date).num_days()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CalendarConfig;

    fn mon_fri_config() -> CalendarConfig {
        CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(), // Monday
            working_days_per_week: 5,
            non_working_dates: vec![],
        }
    }

    #[test]
    fn day_zero_is_start_date() {
        let cfg = mon_fri_config();
        assert_eq!(day_to_date(0, &cfg), cfg.start_date);
    }

    #[test]
    fn five_days_skips_weekend() {
        let cfg = mon_fri_config();
        // Mon Jan 6 + 5 working days = Mon Jan 13
        let result = day_to_date(5, &cfg);
        assert_eq!(result, NaiveDate::from_ymd_opt(2025, 1, 13).unwrap());
    }

    #[test]
    fn one_day_from_friday_is_monday() {
        let cfg = CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 10).unwrap(), // Friday
            working_days_per_week: 5,
            non_working_dates: vec![],
        };
        let result = day_to_date(1, &cfg);
        assert_eq!(result, NaiveDate::from_ymd_opt(2025, 1, 13).unwrap()); // Monday
    }

    #[test]
    fn holiday_is_skipped() {
        let holiday = NaiveDate::from_ymd_opt(2025, 1, 7).unwrap(); // Tuesday
        let cfg = CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(), // Monday
            working_days_per_week: 5,
            non_working_dates: vec![holiday],
        };
        // 1 working day after Mon Jan 6, skipping Tue Jan 7 → Wed Jan 8
        assert_eq!(
            day_to_date(1, &cfg),
            NaiveDate::from_ymd_opt(2025, 1, 8).unwrap()
        );
    }

    #[test]
    fn six_day_week_includes_saturday() {
        let cfg = CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 10).unwrap(), // Friday
            working_days_per_week: 6,
            non_working_dates: vec![],
        };
        // 1 working day after Friday → Saturday (included in 6-day week)
        assert_eq!(
            day_to_date(1, &cfg),
            NaiveDate::from_ymd_opt(2025, 1, 11).unwrap()
        );
    }

    #[test]
    fn date_to_day_roundtrip() {
        let cfg = mon_fri_config();
        for day in [0, 1, 5, 10, 20] {
            let date = day_to_date(day, &cfg);
            assert_eq!(date_to_day(date, &cfg), day, "roundtrip failed for day {day}");
        }
    }

    #[test]
    fn date_to_day_negative() {
        let cfg = CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 13).unwrap(), // Monday
            working_days_per_week: 5,
            non_working_dates: vec![],
        };
        // Mon Jan 6 is 5 working days before Mon Jan 13
        assert_eq!(
            date_to_day(NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(), &cfg),
            -5
        );
    }

    #[test]
    fn effort_to_calendar_days_basic() {
        let cfg = mon_fri_config();
        // 5 working days starting Monday = 7 calendar days (Mon through next Mon)
        assert_eq!(effort_to_calendar_days(5, cfg.start_date, &cfg), 7);
    }
}
