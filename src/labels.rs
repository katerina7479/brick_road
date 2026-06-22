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

/// Formats a timeline day number as a human-readable date label.
/// `month_only` → "Jun 2025";  otherwise → "Jun 16 '25".
fn format_day_label(day: i32, month_only: bool, model: &Model) -> String {
    let date = day_to_date(day, &model.calendar);
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

    let (step, month_only) = day_step_for_zoom(scale);
    let zoom_band_changed = step != *prev_step;

    if !zoom_band_changed && !schedule.is_changed() && !model.is_changed() && !today.is_changed() {
        return;
    }
    *prev_step = step;

    for e in &day_q {
        commands.entity(e).despawn();
    }

    let span = schedule.total_duration_days + step;
    for day in (0..=span).step_by(step as usize) {
        let x = day_to_x(day, &model.calendar);
        let alpha = if day < today.day { 0.3 } else { 0.75 };
        let label = format_day_label(day, month_only, &model);
        commands.spawn((
            DayLabel,
            Text2d::new(label),
            TextFont {
                font_size: 13.0,
                ..default()
            },
            TextColor(Color::srgba(0.6, 0.6, 0.9, alpha)),
            Transform::from_xyz(x, DAY_LABEL_Y, 1.0),
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
    let span_px = day_to_x(span_days, config);
    let start_year = config.start_date.year();
    let mut year = start_year;
    let mut month = config.start_date.month();
    let mut ranges = Vec::new();

    loop {
        let x_start = match first_working_day_of_month(year, month, config) {
            Some(d) => day_to_x(date_to_day(d, config), config).max(0.0),
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
            Some(d) => day_to_x(date_to_day(d, config), config).min(span_px),
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
    use crate::model::Model;

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
        let mut model = Model::default();
        model.calendar = mon_config();
        assert_eq!(format_day_label(0, false, &model), "Jun 16 '25");
    }

    #[test]
    fn format_day_label_five_working_days_is_next_monday() {
        let mut model = Model::default();
        model.calendar = mon_config();
        // 5 working days from Mon Jun 16 = Mon Jun 23
        assert_eq!(format_day_label(5, false, &model), "Jun 23 '25");
    }

    #[test]
    fn format_day_label_month_only_shows_abbreviated_month_and_year() {
        let mut model = Model::default();
        model.calendar = mon_config();
        assert_eq!(format_day_label(0, true, &model), "Jun 2025");
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
}
