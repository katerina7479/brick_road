/// Horizontal scale: screen pixels per working day.
/// At 20px/day and the default 1400×700 window (1180px timeline width after
/// the side panel), the default zoom shows ~59 working days (~12 weeks) —
/// a natural horizon for project planning.
pub const PIXELS_PER_DAY: f32 = 20.0;

/// Vertical distance between block rows in pixels.
pub const ROW_HEIGHT: f32 = 40.0;

/// Width of the egui side panel in logical pixels.
/// Must match the `.min_width()` call in `side_panel_ui`.
pub const SIDE_PANEL_WIDTH: f32 = 220.0;
