//! Multi-document support: a "document" is one `.brickroad` file (a SQLite DB
//! with the standard schema). The app tracks the current document and a
//! most-recent-first list in a plain-text sidecar (`recent_documents` in the
//! per-user data dir — app state, not part of any document's schema). The
//! legacy implicit `brick_road.db` is simply the default document, so existing
//! installs keep opening as before.

use std::path::{Path, PathBuf};

use bevy::prelude::Resource;

/// File extension for brick_road documents. Plain SQLite inside; the open
/// dialog also accepts `.db` for the legacy default file.
pub const DOC_EXTENSION: &str = "brickroad";

/// Maximum entries kept in the recent-documents list.
pub const MAX_RECENTS: usize = 8;

/// The path of the currently open document. The window title and the FILE
/// menu display its stem; `Duplicate…` derives its default name from it.
#[derive(Resource, Debug, Clone)]
pub struct CurrentDocument(pub PathBuf);

/// A requested document operation, applied by the exclusive
/// `apply_document_request` system in `main.rs` at a safe point in the frame.
#[derive(Debug, Clone)]
pub enum DocRequest {
    /// Open an existing document.
    Open(PathBuf),
    /// Create a fresh blank document (one empty plan) at the path and open it.
    New(PathBuf),
    /// Write the current model to a new file and continue working in it.
    Duplicate(PathBuf),
}

/// One-shot mailbox the FILE menu writes and the apply system drains.
#[derive(Resource, Default)]
pub struct PendingDocument(pub Option<DocRequest>);

/// Whether the FILE menu popup is open, plus the click-outside arming flag
/// (false on the frame it opened so the opening click doesn't dismiss it).
#[derive(Resource, Default)]
pub struct FileMenuState {
    pub open: bool,
    pub armed: bool,
}

/// The per-user application data directory (`~/Library/Application Support/…`
/// on macOS). Falls back to the cwd when no home directory resolves.
pub fn app_data_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "katerina7479", "brick_road")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Path of the recent-documents sidecar (one absolute path per line, most
/// recent first; the first line is the document to reopen at launch).
fn recents_path() -> PathBuf {
    app_data_dir().join("recent_documents")
}

/// The recent-documents list, most recent first. Entries whose files no
/// longer exist are dropped.
pub fn load_recents() -> Vec<PathBuf> {
    let Ok(content) = std::fs::read_to_string(recents_path()) else {
        return Vec::new();
    };
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .collect()
}

/// Moves `path` to the front of `list`, deduplicating and capping at `cap`.
/// Pure kernel of [`remember_document`].
pub fn merge_recent(mut list: Vec<PathBuf>, path: &Path, cap: usize) -> Vec<PathBuf> {
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(cap);
    list
}

/// Records `path` as the most recent document (write-through to the sidecar).
pub fn remember_document(path: &Path) {
    if let Err(e) = std::fs::create_dir_all(app_data_dir()) {
        bevy::log::warn!("could not create data dir: {e}");
    }
    let list = merge_recent(load_recents(), path, MAX_RECENTS);
    let content = list
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    if let Err(e) = std::fs::write(recents_path(), content) {
        bevy::log::warn!("could not write recent-documents list: {e}");
    }
}

/// Default width (px) of the right fly-out (settings + block inspector share
/// the slot and the dragged width).
pub const FLYOUT_DEFAULT_WIDTH: f32 = 272.0;
/// Resize range for the right fly-out: narrow enough to tuck away, wide
/// enough for long names/descriptions without swallowing the canvas.
pub const FLYOUT_MIN_WIDTH: f32 = 240.0;
pub const FLYOUT_MAX_WIDTH: f32 = 560.0;

/// Current width of the right fly-out. Seeded from the sidecar at startup;
/// panels write the live width back after layout and `persist_flyout_width`
/// saves it once the drag ends. App state, not document schema.
#[derive(Resource)]
pub struct FlyoutWidth(pub f32);

impl Default for FlyoutWidth {
    fn default() -> Self {
        Self(FLYOUT_DEFAULT_WIDTH)
    }
}

/// Clamps a stored/dragged fly-out width to the legal range; non-finite
/// values fall back to the default.
pub fn clamp_flyout_width(w: f32) -> f32 {
    if w.is_finite() {
        w.clamp(FLYOUT_MIN_WIDTH, FLYOUT_MAX_WIDTH)
    } else {
        FLYOUT_DEFAULT_WIDTH
    }
}

fn flyout_width_path() -> PathBuf {
    app_data_dir().join("flyout_width")
}

/// The persisted fly-out width, clamped; the default when unset/unreadable.
pub fn load_flyout_width() -> f32 {
    std::fs::read_to_string(flyout_width_path())
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(clamp_flyout_width)
        .unwrap_or(FLYOUT_DEFAULT_WIDTH)
}

/// Persists the fly-out width sidecar.
pub fn save_flyout_width(w: f32) {
    if let Err(e) = std::fs::write(flyout_width_path(), format!("{w:.1}")) {
        bevy::log::warn!("could not write flyout width: {e}");
    }
}

/// The display name for a document path: its file stem.
pub fn doc_display_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string())
}

/// Ensures a save-dialog result carries the document extension, so a typed
/// bare name like "remodel" becomes "remodel.brickroad".
pub fn with_doc_extension(path: PathBuf) -> PathBuf {
    if path.extension().is_none() {
        path.with_extension(DOC_EXTENSION)
    } else {
        path
    }
}

/// Seeds a fresh document's model: a single empty plan named after the file,
/// inheriting the current document's settings — the calendar (working days
/// per week, holidays, quarter colors, start date) and the t-shirt size map —
/// so a new project starts from your conventions rather than factory defaults
/// (#323). Work stays per-document: blocks, plans, dependencies, and resources
/// are NOT copied. The plan is non-empty in `plans`, so the demo seeder never
/// fires for it.
pub fn blank_document_model(path: &Path, template: &crate::model::Model) -> crate::model::Model {
    let mut model = crate::model::Model::default();
    model.calendar = template.calendar.clone();
    model.t_shirt_sizes = template.t_shirt_sizes.clone();
    model.create_plan(doc_display_name(path), None);
    model
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_recent_fronts_dedupes_and_caps() {
        let p = |s: &str| PathBuf::from(s);
        // New entry goes to the front.
        let list = merge_recent(vec![p("/a"), p("/b")], &p("/c"), 8);
        assert_eq!(list, vec![p("/c"), p("/a"), p("/b")]);
        // Re-opening an existing entry moves it to the front, no duplicate.
        let list = merge_recent(vec![p("/a"), p("/b"), p("/c")], &p("/b"), 8);
        assert_eq!(list, vec![p("/b"), p("/a"), p("/c")]);
        // The cap trims the oldest.
        let list = merge_recent(vec![p("/a"), p("/b"), p("/c")], &p("/d"), 3);
        assert_eq!(list, vec![p("/d"), p("/a"), p("/b")]);
    }

    #[test]
    fn with_doc_extension_only_fills_missing() {
        assert_eq!(
            with_doc_extension(PathBuf::from("/x/remodel")),
            PathBuf::from("/x/remodel.brickroad")
        );
        assert_eq!(
            with_doc_extension(PathBuf::from("/x/legacy.db")),
            PathBuf::from("/x/legacy.db")
        );
    }

    #[test]
    fn blank_document_has_one_empty_plan_named_after_file() {
        let m = blank_document_model(
            Path::new("/plans/remodel.brickroad"),
            &crate::model::Model::default(),
        );
        assert_eq!(m.plans.len(), 1);
        let plan = m.plans.values().next().unwrap();
        assert_eq!(plan.name, "remodel");
        assert!(plan.root_blocks.is_empty());
        assert!(m.work_blocks.is_empty());
    }

    #[test]
    fn blank_document_inherits_settings_but_not_work() {
        use crate::model::{NonWorkingDate, ResourceType, TShirtSize};
        let mut template = crate::model::Model::default();
        let plan = template.create_plan("old", None);
        template.add_block_to_plan(plan, "work", 0, 5, 0);
        template.set_resource_kind("Team A", ResourceType::Team);
        template.calendar.working_days_per_week = 4;
        template.calendar.non_working_dates.push(NonWorkingDate {
            date: chrono::NaiveDate::from_ymd_opt(2026, 12, 25).unwrap(),
            description: "Christmas".to_string(),
        });
        template.t_shirt_sizes = vec![TShirtSize {
            label: "Sprint".to_string(),
            days: 10,
        }];

        let m = blank_document_model(Path::new("/plans/next.brickroad"), &template);

        // Settings, holidays, and sizes carry over…
        assert_eq!(m.calendar.working_days_per_week, 4);
        assert_eq!(m.calendar.non_working_dates.len(), 1);
        assert_eq!(m.t_shirt_sizes.len(), 1);
        assert_eq!(m.t_shirt_sizes[0].label, "Sprint");
        // …work and resources do not.
        assert!(m.work_blocks.is_empty());
        assert!(m.resource_blocks.is_empty());
        assert_eq!(m.plans.len(), 1, "just the new empty plan");
        assert_eq!(m.plans.values().next().unwrap().name, "next");
    }

    #[test]
    fn clamp_flyout_width_bounds_and_rejects_nonfinite() {
        assert_eq!(clamp_flyout_width(300.0), 300.0);
        assert_eq!(clamp_flyout_width(10.0), FLYOUT_MIN_WIDTH);
        assert_eq!(clamp_flyout_width(9000.0), FLYOUT_MAX_WIDTH);
        assert_eq!(clamp_flyout_width(f32::NAN), FLYOUT_DEFAULT_WIDTH);
        assert_eq!(clamp_flyout_width(f32::INFINITY), FLYOUT_DEFAULT_WIDTH);
    }

    #[test]
    fn doc_display_name_uses_stem() {
        assert_eq!(
            doc_display_name(Path::new("/a/b/remodel.brickroad")),
            "remodel"
        );
        assert_eq!(
            doc_display_name(Path::new("/a/b/brick_road.db")),
            "brick_road"
        );
    }
}
