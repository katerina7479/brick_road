use std::collections::HashSet;

use chrono::{Datelike, Duration, NaiveDate};

use crate::constants::PIXELS_PER_DAY;
use crate::model::{CalendarConfig, Day};

// ── Holiday-aware horizontal layout ─────────────────────────────────────────
//
// The timeline axis is working days: weekends take no horizontal space. A
// holiday, however, is shown as its own greyed day-wide column that work skips.
// So the on-screen x of a working day is its working-day index *plus* the number
// of holiday columns inserted before it. The model/scheduler stay in pure
// working days; only this layer (and its inverse `x_to_day`) know about columns.

impl CalendarConfig {
    /// The project-wide non-working dates as a set, ready for the holiday-aware
    /// day→pixel math below. This is the *global* layer every timeline caller
    /// uses; per-resource off-days (br-217) are unioned on top by callers that
    /// render one resource's row.
    pub fn global_off_days(&self) -> HashSet<NaiveDate> {
        self.non_working_dates.iter().map(|nwd| nwd.date).collect()
    }
}

/// A holiday inserts a column only if it lands on what would otherwise be a
/// working weekday (a holiday already on a weekend is compressed away like any
/// weekend). Its column sits just before the working day that follows it —
/// `date_to_day(holiday) + 1`. Returns those boundaries, ascending.
///
/// `non_working` is supplied explicitly (rather than read from `config`) so a
/// caller can pass the global holidays alone or unioned with a resource's
/// off-days; `config` is still needed for the working week and `date_to_day`.
fn holiday_boundaries(non_working: &HashSet<NaiveDate>, config: &CalendarConfig) -> Vec<Day> {
    let mut b: Vec<Day> = non_working
        .iter()
        .filter(|d| d.weekday().number_from_monday() <= config.working_days_per_week as u32)
        .map(|d| date_to_day(*d, config) + 1)
        .collect();
    b.sort_unstable();
    b
}

/// Number of holiday columns inserted before working day `day`, given the
/// explicit non-working set.
pub fn holiday_offset(day: Day, non_working: &HashSet<NaiveDate>, config: &CalendarConfig) -> i32 {
    if non_working.is_empty() {
        return 0;
    }
    holiday_boundaries(non_working, config)
        .iter()
        .filter(|&&b| b <= day)
        .count() as i32
}

/// World-space x of the left edge of working day `day`, accounting for the
/// greyed holiday columns (from `non_working`) inserted before it.
pub fn day_to_x(day: Day, non_working: &HashSet<NaiveDate>, config: &CalendarConfig) -> f32 {
    (day + holiday_offset(day, non_working, config)) as f32 * PIXELS_PER_DAY
}

/// Inverse of [`day_to_x`]: the working day whose column contains world x `x`
/// (floored). An x that lands inside a holiday column resolves to the working
/// day just before that column.
pub fn x_to_day(x: f32, non_working: &HashSet<NaiveDate>, config: &CalendarConfig) -> Day {
    let v = (x / PIXELS_PER_DAY).floor() as Day; // visual column index
    if non_working.is_empty() {
        return v;
    }
    // Largest working day whose column starts at or before `v`. `day +
    // holiday_offset(day)` is monotonic, so walk down from `v` until it fits.
    let mut day = v;
    while day + holiday_offset(day, non_working, config) > v {
        day -= 1;
    }
    day
}

/// `(left_x, date)` for each holiday column whose working-day boundary falls in
/// `0..=span_days`. Consecutive holidays that share a boundary get adjacent
/// columns (earliest date leftmost). Reads the global holidays (with their
/// descriptions) from `config`; the internal `day_to_x` calls use the global
/// off-day set.
pub fn holiday_columns(config: &CalendarConfig, span_days: Day) -> Vec<(f32, NaiveDate, String)> {
    let non_working = config.global_off_days();
    let mut holidays: Vec<(NaiveDate, String)> = config
        .non_working_dates
        .iter()
        .filter(|nwd| {
            nwd.date.weekday().number_from_monday() <= config.working_days_per_week as u32
        })
        .map(|nwd| (nwd.date, nwd.description.clone()))
        .collect();
    holidays.sort_unstable_by_key(|(d, _)| *d);

    let mut out = Vec::new();
    let mut i = 0;
    while i < holidays.len() {
        let boundary = date_to_day(holidays[i].0, config) + 1;
        // Gather the run of holidays sharing this boundary (adjacent days).
        let mut group = vec![holidays[i].clone()];
        let mut j = i + 1;
        while j < holidays.len() && date_to_day(holidays[j].0, config) + 1 == boundary {
            group.push(holidays[j].clone());
            j += 1;
        }
        if boundary >= 0 && boundary <= span_days + 1 {
            // The group occupies the columns immediately left of working day
            // `boundary`; earliest holiday leftmost.
            let right = day_to_x(boundary, &non_working, config);
            let n = group.len();
            for (k, (date, desc)) in group.into_iter().enumerate() {
                let left_x = right - (n - k) as f32 * PIXELS_PER_DAY;
                out.push((left_x, date, desc));
            }
        }
        i = j;
    }
    out
}

/// Returns true if `date` is a working day under `config`.
///
/// A day is non-working if:
/// - Its ISO weekday number (Mon=1 … Sun=7) exceeds `working_days_per_week`, or
/// - It appears in `config.non_working_dates`.
fn is_working_day(
    date: NaiveDate,
    working_days_per_week: u8,
    non_working: &HashSet<NaiveDate>,
) -> bool {
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
    let non_working: HashSet<NaiveDate> = config
        .non_working_dates
        .iter()
        .map(|nwd| nwd.date)
        .collect();

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

    let non_working: HashSet<NaiveDate> = config
        .non_working_dates
        .iter()
        .map(|nwd| nwd.date)
        .collect();

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

/// The timeline day for the "today" marker on `date`.
///
/// On a working day this is the start of that day's own cell. On a non-working
/// day (weekend/holiday) it advances one past the last completed working day, so
/// the marker sits *after* the finished work week (e.g. a Saturday lands just
/// after Friday, not on it).
pub fn today_marker_day(date: NaiveDate, config: &CalendarConfig) -> Day {
    let non_working: HashSet<NaiveDate> = config
        .non_working_dates
        .iter()
        .map(|nwd| nwd.date)
        .collect();
    let base = date_to_day(date, config);
    if is_working_day(date, config.working_days_per_week, &non_working) {
        base
    } else {
        base + 1
    }
}

/// Returns the first calendar day in (year, month) that is a working day under `config`.
/// Returns `None` if the month contains no working days.
pub fn first_working_day_of_month(
    year: i32,
    month: u32,
    config: &CalendarConfig,
) -> Option<NaiveDate> {
    let non_working: std::collections::HashSet<NaiveDate> = config
        .non_working_dates
        .iter()
        .map(|nwd| nwd.date)
        .collect();
    let mut day = NaiveDate::from_ymd_opt(year, month, 1)?;
    loop {
        if day.month() != month {
            return None;
        }
        if is_working_day(day, config.working_days_per_week, &non_working) {
            return Some(day);
        }
        day = day.checked_add_signed(Duration::days(1))?;
    }
}

/// Converts a UTC Unix timestamp (seconds since the Unix epoch) to a
/// Gregorian calendar date using the Howard Hinnant civil-from-days algorithm
/// (public domain). Falls back to 2025-01-01 on malformed arithmetic (cannot
/// occur with timestamps produced by `SystemTime::now`).
pub fn unix_secs_to_date(secs: u64) -> NaiveDate {
    let z = (secs / 86400) as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe as i64 + era * 400 + if m <= 2 { 1 } else { 0 };
    NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2025, 1, 1).unwrap())
}

/// Converts effort in working days to a calendar duration in calendar days.
///
/// Returns the number of calendar days from `start_date` to complete
/// `effort_days` working days of work.
pub fn effort_to_calendar_days(
    effort_days: Day,
    start_date: NaiveDate,
    config: &CalendarConfig,
) -> i64 {
    let finish = day_to_date(
        effort_days,
        &CalendarConfig {
            start_date,
            ..config.clone()
        },
    );
    (finish - start_date).num_days()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CalendarConfig, NonWorkingDate};

    fn mon_fri_config() -> CalendarConfig {
        CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(), // Monday
            working_days_per_week: 5,
            non_working_dates: vec![],
            ..Default::default()
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
            ..Default::default()
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
            non_working_dates: vec![NonWorkingDate {
                date: holiday,
                description: String::new(),
            }],
            ..Default::default()
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
            ..Default::default()
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
            assert_eq!(
                date_to_day(date, &cfg),
                day,
                "roundtrip failed for day {day}"
            );
        }
    }

    #[test]
    fn date_to_day_negative() {
        let cfg = CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 13).unwrap(), // Monday
            working_days_per_week: 5,
            non_working_dates: vec![],
            ..Default::default()
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

    #[test]
    fn day_to_x_is_identity_without_holidays() {
        let cfg = mon_fri_config();
        let off = cfg.global_off_days();
        for d in [0, 1, 5, 20] {
            assert_eq!(day_to_x(d, &off, &cfg), d as f32 * PIXELS_PER_DAY);
            assert_eq!(x_to_day(day_to_x(d, &off, &cfg), &off, &cfg), d);
        }
    }

    #[test]
    fn holiday_inserts_a_column_after_its_boundary() {
        // Mon Jan 6 start; make Wed Jan 8 (working day 2) a holiday.
        let holiday = NaiveDate::from_ymd_opt(2025, 1, 8).unwrap();
        let cfg = CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(),
            working_days_per_week: 5,
            non_working_dates: vec![NonWorkingDate {
                date: holiday,
                description: String::new(),
            }],
            ..Default::default()
        };
        let off = cfg.global_off_days();
        // date_to_day(Jan 8) = 1 (Tue Jan 7 is working day 1), boundary = 2.
        // Days before the boundary are unshifted; day 2 onward shift one column.
        assert_eq!(day_to_x(1, &off, &cfg), 1.0 * PIXELS_PER_DAY);
        assert_eq!(day_to_x(2, &off, &cfg), 3.0 * PIXELS_PER_DAY);
        // The holiday column sits between them, at visual index 2.
        let cols = holiday_columns(&cfg, 20);
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].0, 2.0 * PIXELS_PER_DAY);
        assert_eq!(cols[0].1, holiday);
        // Inverse: an x in the holiday column resolves to the prior working day.
        assert_eq!(x_to_day(2.5 * PIXELS_PER_DAY, &off, &cfg), 1);
        assert_eq!(x_to_day(3.0 * PIXELS_PER_DAY, &off, &cfg), 2);
    }

    #[test]
    fn weekend_holiday_inserts_no_column() {
        // A holiday already on a Saturday is compressed like any weekend.
        let sat = NaiveDate::from_ymd_opt(2025, 1, 11).unwrap();
        let cfg = CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(),
            working_days_per_week: 5,
            non_working_dates: vec![NonWorkingDate {
                date: sat,
                description: String::new(),
            }],
            ..Default::default()
        };
        let off = cfg.global_off_days();
        assert!(holiday_columns(&cfg, 20).is_empty());
        assert_eq!(day_to_x(5, &off, &cfg), 5.0 * PIXELS_PER_DAY);
    }

    #[test]
    fn explicit_extra_off_day_shifts_day_to_x_by_one_column() {
        // The seam br-217 relies on: passing a set with an extra working-weekday
        // date inserts one greyed column, independent of `config.non_working_dates`
        // (which is left empty here). The column lands just before the working day
        // *after* the extra date. Since `config` has no holidays, the extra date
        // (Wed Jan 8) is itself global working day 2, so its boundary is day 3 and
        // days 3+ shift one column right; days 0–2 are untouched.
        let cfg = mon_fri_config(); // Mon Jan 6 start, no global holidays
        let global = cfg.global_off_days();
        assert!(global.is_empty());

        let mut augmented = global.clone();
        augmented.insert(NaiveDate::from_ymd_opt(2025, 1, 8).unwrap());

        // Days before the boundary are identical to the unaugmented layout.
        assert_eq!(
            day_to_x(2, &augmented, &cfg),
            day_to_x(2, &global, &cfg),
            "day 2 (before the inserted column) is unshifted"
        );
        // Day 3 onward shifts right by exactly one greyed column.
        assert_eq!(
            day_to_x(3, &augmented, &cfg),
            day_to_x(3, &global, &cfg) + PIXELS_PER_DAY,
            "day 3 shifts one column for the explicit off-day"
        );
        // The explicit set never mutates the calendar config.
        assert_eq!(cfg.non_working_dates, Vec::new());
    }

    #[test]
    fn unix_secs_to_date_epoch_is_jan_1_1970() {
        assert_eq!(
            unix_secs_to_date(0),
            NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()
        );
    }

    #[test]
    fn unix_secs_to_date_one_day() {
        assert_eq!(
            unix_secs_to_date(86400),
            NaiveDate::from_ymd_opt(1970, 1, 2).unwrap()
        );
    }

    #[test]
    fn unix_secs_to_date_leap_day_2000() {
        // 2000-02-29: 11016 days after epoch. 946684800 = 2000-01-01, plus 59 days.
        let secs = 946684800 + 59 * 86400;
        assert_eq!(
            unix_secs_to_date(secs),
            NaiveDate::from_ymd_opt(2000, 2, 29).unwrap()
        );
    }

    #[test]
    fn unix_secs_to_date_modern_date() {
        // 2025-01-06 00:00:00 UTC = 1736121600
        assert_eq!(
            unix_secs_to_date(1_736_121_600),
            NaiveDate::from_ymd_opt(2025, 1, 6).unwrap()
        );
    }

    #[test]
    fn today_marker_advances_past_finished_week_on_weekend() {
        // start_date is a Monday; Fri is working day 4.
        let cfg = mon_fri_config();
        let fri = NaiveDate::from_ymd_opt(2025, 1, 17).unwrap(); // Mon Jan 13 + 4
        let sat = NaiveDate::from_ymd_opt(2025, 1, 18).unwrap();
        let sun = NaiveDate::from_ymd_opt(2025, 1, 19).unwrap();
        // A working day marks its own cell.
        assert_eq!(today_marker_day(fri, &cfg), date_to_day(fri, &cfg));
        // A weekend sits one past the last completed working day (after Friday).
        let after_fri = date_to_day(fri, &cfg) + 1;
        assert_eq!(today_marker_day(sat, &cfg), after_fri);
        assert_eq!(today_marker_day(sun, &cfg), after_fri);
    }
}
