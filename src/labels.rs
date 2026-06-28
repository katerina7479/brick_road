use bevy::prelude::*;
use chrono::Datelike;

use crate::{
    calendar::{date_to_day, day_to_date, day_to_x, first_working_day_of_month},
    model::Model,
    schedule::Schedule,
};

/// Y position of day-number labels above the block rows.
const DAY_LABEL_Y: f32 = 30.0;

/// Maps orthographic zoom scale to (stride_days, use_month_format).
/// `stride_days` is the gap between labels; `use_month_format` switches to
/// "Mon YYYY" at far zoom where individual dates are too dense to read.
fn day_step_for_zoom(scale: f32) -> (i32, bool) {
    // Thresholds tuned for PIXELS_PER_DAY=20:
    //   scale < 0.25 → zoomed in enough that daily labels fit (1 day ≥ 80px)
    //   scale < 1.0  → weekly stride (1 day 20–80px)
    //   scale < 3.0  → bi-weekly stride
    //   scale ≥ 3.0  → monthly (1 day < 7px, individual dates unreadable)
    if scale < 0.25 {
        (1, false)
    } else if scale < 1.0 {
        (5, false)
    } else if scale < 3.0 {
        (10, false)
    } else {
        (30, true)
    }
}

/// Minimum on-screen horizontal spacing (px) between adjacent day *numbers* in
/// the calendar ruler — about a two-digit glyph plus breathing room.
const MIN_DAY_LABEL_PX: f32 = 22.0;

/// How many days to skip between drawn day *numbers* in the calendar ruler so
/// adjacent numbers keep a comfortable gap, given the on-screen width of one day
/// (`day_w`, px). The per-day ticks stay dense (they read as a fine grid); only
/// the numbers thin out. `day_w >= 22` -> stride 1 (every day, when there's room);
/// narrower columns stride 2, 3, ... so the numbers never touch.
pub(crate) fn day_label_stride(day_w: f32) -> i32 {
    (MIN_DAY_LABEL_PX / day_w).ceil().max(1.0) as i32
}

/// Formats a timeline day number as a human-readable date label.
/// `month_only` → "Jun 2025";  otherwise → "Jun 16 '25".
fn format_day_label(day: i32, month_only: bool, config: &crate::model::CalendarConfig) -> String {
    let date = day_to_date(day, config);
    if month_only {
        format!("{} {}", date.format("%b"), date.year())
    } else {
        format!(
            "{} {} '{:02}",
            date.format("%b"),
            date.day(),
            date.year() % 100
        )
    }
}

/// Marker for day-number `Text2d` entities.
#[derive(Component)]
pub struct DayLabel;

/// One entry produced by [`compute_day_labels`]: a day, its world-x, its text,
/// and an alpha (past days are dimmed to 0.3; future/today stay at 0.75).
pub(crate) struct DayLabelEntry {
    #[allow(dead_code)]
    pub day: crate::model::Day,
    pub x: f32,
    pub label: String,
    pub alpha: f32,
}

/// Pure computation: the ordered list of day labels for a timeline. Extracted
/// from [`spawn_day_labels`] so the selection and placement logic can be unit-
/// tested without a Bevy world, mirroring [`compute_quarter_ranges`].
///
/// `total_duration_days` is the timeline span (from `Schedule`).
/// `scale` is the current orthographic zoom (from the camera projection).
/// `today_day` is the working-day offset of "today" for alpha dimming.
pub(crate) fn compute_day_labels(
    config: &crate::model::CalendarConfig,
    total_duration_days: crate::model::Day,
    scale: f32,
    today_day: crate::model::Day,
) -> Vec<DayLabelEntry> {
    let (step, month_only) = day_step_for_zoom(scale);
    let span = total_duration_days + step;
    let off = config.global_off_days();
    (0..=span)
        .step_by(step as usize)
        .map(|day| DayLabelEntry {
            day,
            x: day_to_x(day, &off, config),
            label: format_day_label(day, month_only, config),
            alpha: if day < today_day { 0.3 } else { 0.75 },
        })
        .collect()
}

/// Spawns (or re-spawns) day-number labels along the top of the timeline.
///
/// Respawns when:
/// - The zoom band changes (scale crosses one of the 0.5 / 2.0 / 4.0 thresholds).
/// - `model` or `schedule` changes (timeline span may have shifted).
///
/// Uses a `Local<i32>` to track the previously-active stride so that smooth
/// zooming within a band incurs no per-frame entity churn.
pub fn spawn_day_labels(
    mut commands: Commands,
    schedule: Res<Schedule>,
    model: Res<Model>,
    today: Res<crate::schedule::TodayMarker>,
    cam_q: Query<&Projection, With<Camera2d>>,
    day_q: Query<Entity, With<DayLabel>>,
    mut prev_step: Local<i32>,
) {
    let scale = cam_q
        .single()
        .ok()
        .and_then(|proj| {
            if let Projection::Orthographic(o) = proj {
                Some(o.scale)
            } else {
                None
            }
        })
        .unwrap_or(1.0);

    let (step, _) = day_step_for_zoom(scale);
    let zoom_band_changed = step != *prev_step;

    if !zoom_band_changed && !schedule.is_changed() && !model.is_changed() && !today.is_changed() {
        return;
    }
    *prev_step = step;

    for e in &day_q {
        commands.entity(e).despawn();
    }

    for entry in compute_day_labels(
        &model.calendar,
        schedule.total_duration_days,
        scale,
        today.day,
    ) {
        commands.spawn((
            DayLabel,
            Text2d::new(entry.label),
            TextFont {
                font_size: 13.0,
                ..default()
            },
            TextColor(Color::srgba(0.6, 0.6, 0.9, entry.alpha)),
            Transform::from_xyz(entry.x, DAY_LABEL_Y, 1.0),
        ));
    }
}

/// Marker for quarter/period label `Text2d` entities.
#[derive(Component)]
pub struct PeriodLabel;

const PERIOD_LABEL_Y: f32 = 48.0;

/// One entry produced by [`compute_quarter_ranges`]: a quarter label and its
/// horizontal centre in world space.
pub(crate) struct QuarterRange {
    pub label: String,
    pub cx: f32,
}

/// Pure computation: builds the ordered list of quarter labels and their
/// centres for a timeline that starts at `config.start_date` and spans
/// `span_days` working days. Called by [`spawn_period_labels`]; extracted
/// here so the loop logic can be unit-tested without a Bevy world.
pub(crate) fn compute_quarter_ranges(
    config: &crate::model::CalendarConfig,
    span_days: crate::model::Day,
) -> Vec<QuarterRange> {
    let off = config.global_off_days();
    let span_px = day_to_x(span_days, &off, config);
    let start_year = config.start_date.year();
    let mut year = start_year;
    let mut month = config.start_date.month();
    let mut ranges = Vec::new();

    loop {
        let x_start = match first_working_day_of_month(year, month, config) {
            Some(d) => day_to_x(date_to_day(d, config), &off, config).max(0.0),
            None => {
                let (ny, nm) = next_ym(year, month);
                year = ny;
                month = nm;
                if year > start_year + 50 {
                    break;
                }
                continue;
            }
        };
        if x_start >= span_px {
            break;
        }

        let quarter = (month - 1) / 3 + 1;
        let q_end_month = quarter * 3 + 1;
        let (q_end_year, q_end_mon) = if q_end_month > 12 {
            (year + 1, 1)
        } else {
            (year, q_end_month)
        };
        let x_end = match first_working_day_of_month(q_end_year, q_end_mon, config) {
            Some(d) => day_to_x(date_to_day(d, config), &off, config).min(span_px),
            None => span_px,
        };

        ranges.push(QuarterRange {
            label: format!("Q{} '{:02}", quarter, year % 100),
            cx: (x_start + x_end) * 0.5,
        });

        month = q_end_mon;
        year = q_end_year;
    }
    ranges
}

/// Spawns (or re-spawns) quarter labels ("Q1 '25", "Q2 '25") above the day labels.
/// Fires when model or schedule changes.
pub fn spawn_period_labels(
    mut commands: Commands,
    schedule: Res<Schedule>,
    model: Res<Model>,
    label_q: Query<Entity, With<PeriodLabel>>,
) {
    if !model.is_changed() && !schedule.is_changed() {
        return;
    }
    for e in &label_q {
        commands.entity(e).despawn();
    }

    let span_days = schedule.total_duration_days + 30;
    for qr in compute_quarter_ranges(&model.calendar, span_days) {
        commands.spawn((
            PeriodLabel,
            Text2d::new(qr.label),
            TextFont {
                font_size: 11.0,
                ..default()
            },
            TextColor(Color::srgba(0.65, 0.65, 0.85, 0.70)),
            Transform::from_xyz(qr.cx, PERIOD_LABEL_Y, 1.0),
        ));
    }
}

fn next_ym(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

pub fn scale_labels_to_zoom(
    cam_q: Query<&Projection, With<Camera2d>>,
    mut label_q: Query<&mut Transform, With<DayLabel>>,
) {
    let Ok(proj) = cam_q.single() else { return };
    let Projection::Orthographic(ortho) = proj else {
        return;
    };
    let s = ortho.scale;
    for mut transform in &mut label_q {
        transform.scale = Vec3::splat(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon_config() -> crate::model::CalendarConfig {
        crate::model::CalendarConfig {
            start_date: chrono::NaiveDate::from_ymd_opt(2025, 6, 16).unwrap(), // Monday
            working_days_per_week: 5,
            non_working_dates: vec![],
            ..Default::default()
        }
    }

    #[test]
    fn format_day_label_day_zero_shows_start_date() {
        let cfg = mon_config();
        assert_eq!(format_day_label(0, false, &cfg), "Jun 16 '25");
    }

    #[test]
    fn format_day_label_five_working_days_is_next_monday() {
        let cfg = mon_config();
        // 5 working days from Mon Jun 16 = Mon Jun 23
        assert_eq!(format_day_label(5, false, &cfg), "Jun 23 '25");
    }

    #[test]
    fn format_day_label_month_only_shows_abbreviated_month_and_year() {
        let cfg = mon_config();
        assert_eq!(format_day_label(0, true, &cfg), "Jun 2025");
    }

    #[test]
    fn next_ym_increments_month() {
        assert_eq!(next_ym(2025, 1), (2025, 2));
        assert_eq!(next_ym(2025, 11), (2025, 12));
    }

    #[test]
    fn next_ym_wraps_december_to_january_of_next_year() {
        assert_eq!(next_ym(2025, 12), (2026, 1));
        assert_eq!(next_ym(1999, 12), (2000, 1));
    }

    #[test]
    fn quarter_ranges_jan_start_two_years() {
        // Start on a Monday in Jan; span ~500 working days covers ≈2 full years.
        // Expect exactly 8 quarter labels: Q1..Q4 '25, Q1..Q4 '26.
        let cfg = crate::model::CalendarConfig {
            start_date: chrono::NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(),
            working_days_per_week: 5,
            non_working_dates: vec![],
            ..Default::default()
        };
        let ranges = compute_quarter_ranges(&cfg, 500);
        assert_eq!(ranges.len(), 8, "two full years → 8 quarters");
        assert_eq!(ranges[0].label, "Q1 '25");
        assert_eq!(ranges[3].label, "Q4 '25");
        assert_eq!(ranges[4].label, "Q1 '26");
        assert_eq!(ranges[7].label, "Q4 '26");
    }

    #[test]
    fn quarter_ranges_q4_wraps_to_next_year() {
        // Start in October (Q4). The first label should be Q4 of the start year
        // and the next should be Q1 of the following year.
        let cfg = crate::model::CalendarConfig {
            start_date: chrono::NaiveDate::from_ymd_opt(2025, 10, 6).unwrap(),
            working_days_per_week: 5,
            non_working_dates: vec![],
            ..Default::default()
        };
        let ranges = compute_quarter_ranges(&cfg, 150);
        assert!(!ranges.is_empty());
        assert_eq!(ranges[0].label, "Q4 '25");
        // With 150 working days we cross into 2026; Q1 '26 should appear.
        assert!(
            ranges.iter().any(|r| r.label == "Q1 '26"),
            "Q1 '26 should appear after 150 days starting in Oct 2025"
        );
    }

    #[test]
    fn quarter_ranges_zero_span_is_empty() {
        let cfg = mon_config();
        let ranges = compute_quarter_ranges(&cfg, 0);
        assert!(ranges.is_empty(), "span of 0 should produce no labels");
    }

    #[test]
    fn day_step_for_zoom_close_is_daily_no_month() {
        // scale < 0.25 → daily labels (1 working day ≥ 80px at PIXELS_PER_DAY=20)
        let (step, month_only) = day_step_for_zoom(0.1);
        assert_eq!(step, 1);
        assert!(!month_only);
    }

    #[test]
    fn day_step_for_zoom_far_is_monthly_with_month_format() {
        let (step, month_only) = day_step_for_zoom(5.0);
        assert_eq!(step, 30);
        assert!(month_only);
    }

    #[test]
    fn day_label_stride_thins_as_columns_narrow() {
        assert_eq!(day_label_stride(30.0), 1, "wide columns show every day");
        assert_eq!(
            day_label_stride(22.0),
            1,
            "at the min spacing, still every day"
        );
        assert_eq!(
            day_label_stride(13.0),
            2,
            "cramped columns show every other day"
        );
        assert_eq!(
            day_label_stride(7.0),
            4,
            "very narrow columns show every fourth day"
        );
    }

    // ── compute_day_labels ───────────────────────────────────────────────────

    #[test]
    fn day_labels_daily_stride_at_close_zoom() {
        // scale=0.1 → step=1; with span=5 we expect days 0,1,2,3,4,5,6 (0..=5+1)
        let cfg = mon_config();
        let entries = compute_day_labels(&cfg, 5, 0.1, 100);
        let days: Vec<i32> = entries.iter().map(|e| e.day).collect();
        assert_eq!(days, vec![0, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn day_labels_weekly_stride_at_medium_zoom() {
        // scale=0.5 → step=5; span=20 → days 0,5,10,15,20,25
        let cfg = mon_config();
        let entries = compute_day_labels(&cfg, 20, 0.5, 100);
        let days: Vec<i32> = entries.iter().map(|e| e.day).collect();
        assert_eq!(days, vec![0, 5, 10, 15, 20, 25]);
    }

    #[test]
    fn day_labels_biweekly_stride() {
        // scale=2.0 → step=10; span=20 → days 0,10,20,30
        let cfg = mon_config();
        let entries = compute_day_labels(&cfg, 20, 2.0, 100);
        let days: Vec<i32> = entries.iter().map(|e| e.day).collect();
        assert_eq!(days, vec![0, 10, 20, 30]);
    }

    #[test]
    fn day_labels_monthly_stride_at_far_zoom() {
        // scale=4.0 → step=30, month_only=true; span=60 → days 0,30,60,90
        let cfg = mon_config();
        let entries = compute_day_labels(&cfg, 60, 4.0, 100);
        let days: Vec<i32> = entries.iter().map(|e| e.day).collect();
        assert_eq!(days, vec![0, 30, 60, 90]);
        // All labels should be in "Mon YYYY" format (no day number).
        for e in &entries {
            assert!(
                !e.label.contains(" '"),
                "monthly label should not contain \"'YY\": {}",
                e.label
            );
        }
    }

    #[test]
    fn day_labels_span_includes_one_extra_step_beyond_duration() {
        // span fed to compute_day_labels is total_duration_days + step, so labels
        // extend one stride past the last block.
        let cfg = mon_config();
        let entries = compute_day_labels(&cfg, 10, 0.5, 100); // step=5, span=15
        let last_day = entries.last().unwrap().day;
        assert_eq!(last_day, 15, "last label should be at duration + step");
    }

    #[test]
    fn day_labels_x_positions_match_day_to_x() {
        use crate::calendar::day_to_x;
        let cfg = mon_config();
        let entries = compute_day_labels(&cfg, 10, 0.5, 100);
        let off = cfg.global_off_days();
        for e in &entries {
            let expected_x = day_to_x(e.day, &off, &cfg);
            assert!(
                (e.x - expected_x).abs() < 0.001,
                "day {} x={} expected {}",
                e.day,
                e.x,
                expected_x
            );
        }
    }

    #[test]
    fn day_labels_past_days_dimmed_future_days_bright() {
        // today=5: days 0..4 get alpha=0.3, days ≥5 get alpha=0.75
        let cfg = mon_config();
        let entries = compute_day_labels(&cfg, 10, 0.5, 5); // step=5, today=5
        for e in &entries {
            let expected_alpha = if e.day < 5 { 0.3 } else { 0.75 };
            assert!(
                (e.alpha - expected_alpha).abs() < 0.001,
                "day {} alpha={} expected {}",
                e.day,
                e.alpha,
                expected_alpha
            );
        }
    }

    #[test]
    fn day_labels_zero_span_still_has_day_zero() {
        // Even with total_duration_days=0, span = 0+step so day 0 always appears.
        let cfg = mon_config();
        let entries = compute_day_labels(&cfg, 0, 0.5, 100); // step=5, span=5
        assert!(entries.iter().any(|e| e.day == 0), "day 0 always present");
    }

    #[test]
    fn day_labels_holiday_shifts_x_positions() {
        // A holiday on a weekday inserts an extra visual column, widening all
        // x positions at or after the holiday boundary.
        use crate::model::NonWorkingDate;
        use chrono::NaiveDate;
        let mut cfg = mon_config();
        // 2025-06-18 is a Wednesday (working day 2 in this calendar).
        let holiday = NaiveDate::from_ymd_opt(2025, 6, 18).unwrap();
        cfg.non_working_dates = vec![NonWorkingDate {
            date: holiday,
            description: String::new(),
        }];

        let cfg_no_hol = mon_config();

        let entries_hol = compute_day_labels(&cfg, 10, 0.5, 100); // step=5
        let entries_plain = compute_day_labels(&cfg_no_hol, 10, 0.5, 100);

        // Day 0 is before the holiday column (boundary is at day 3); same x.
        let x0_hol = entries_hol.iter().find(|e| e.day == 0).unwrap().x;
        let x0_plain = entries_plain.iter().find(|e| e.day == 0).unwrap().x;
        assert!(
            (x0_hol - x0_plain).abs() < 0.001,
            "day 0 unaffected by holiday"
        );

        // Day 5 is after the holiday column; shifted right by PIXELS_PER_DAY.
        use crate::constants::PIXELS_PER_DAY;
        let x5_hol = entries_hol.iter().find(|e| e.day == 5).unwrap().x;
        let x5_plain = entries_plain.iter().find(|e| e.day == 5).unwrap().x;
        assert!(
            (x5_hol - x5_plain - PIXELS_PER_DAY).abs() < 0.001,
            "day 5 shifted right by one holiday column: hol={x5_hol} plain={x5_plain}"
        );
    }

    #[test]
    fn day_labels_label_text_matches_format_day_label() {
        // Spot-check that the label strings match format_day_label directly.
        let cfg = mon_config();
        let entries = compute_day_labels(&cfg, 5, 0.5, 100); // step=5, month_only=false
        for e in &entries {
            let expected = format_day_label(e.day, false, &cfg);
            assert_eq!(e.label, expected, "label mismatch at day {}", e.day);
        }
    }
}
