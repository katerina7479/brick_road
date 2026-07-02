/// Horizontal scale: screen pixels per working day.
/// At 20px/day and the default 1400×700 window (1180px timeline width after
/// the side panel), the default zoom shows ~59 working days (~12 weeks) —
/// a natural horizon for project planning.
pub const PIXELS_PER_DAY: f32 = 20.0;

/// Vertical distance between block rows in pixels.
pub const ROW_HEIGHT: f32 = 40.0;

/// The fixed "Events" row above the resource rows (row −1): external
/// targets/milestones live here. It is the upper bound for block placement —
/// drags, pastes, and creation clamp to it, so rows above it are unreachable.
/// It is not a resource: the gutter shows a fixed label with no rename,
/// resource picker, or drag-reorder.
pub const EVENTS_ROW: i32 = -1;
