use std::collections::HashMap;

use crate::calendar;
use crate::model::{Model, Plan, PlanId, WorkBlockId};

const HEADER: &str = "id,name,parent_id,start_date,duration_days,row,row_name\n";

/// Serialises all blocks in `plan` to a flat CSV string.
///
/// One row per block, in DFS pre-order (parent always before its children).
/// Columns:
///   id            — stable numeric block ID; use as a cross-reference key
///   name          — block name (RFC-4180 quoted if it contains commas/quotes)
///   parent_id     — id of the parent block, empty string for top-level blocks
///   start_date    — working-day start converted to a real calendar date (YYYY-MM-DD)
///   duration_days — length in working days
///   row           — zero-indexed lane/resource row in this plan
///   row_name      — user-assigned label for that row, empty if unnamed
pub fn plan_to_csv(plan: &Plan, model: &Model) -> String {
    let mut out = String::from(HEADER);
    for id in blocks_preorder(plan, model) {
        let Some(wb) = model.work_blocks.get(&id) else {
            continue;
        };
        let row = plan.block_rows.get(&id).copied().unwrap_or(0);
        let row_name = plan.row_name(None, row).unwrap_or("").to_string();
        let start_date = calendar::day_to_date(wb.start_day, &model.calendar)
            .format("%Y-%m-%d")
            .to_string();
        let parent_id = wb.parent.map(|p| p.0.to_string()).unwrap_or_default();
        out.push_str(&csv_row(&[
            &id.0.to_string(),
            &wb.name,
            &parent_id,
            &start_date,
            &wb.duration_days.to_string(),
            &row.to_string(),
            &row_name,
        ]));
    }
    out
}

/// DFS pre-order traversal of all blocks reachable from `plan.root_blocks`.
///
/// Children are sorted by start_day then block id for stable, reproducible output.
fn blocks_preorder(plan: &Plan, model: &Model) -> Vec<WorkBlockId> {
    let mut children: HashMap<WorkBlockId, Vec<WorkBlockId>> = HashMap::new();
    for wb in model.work_blocks.values() {
        if let Some(parent) = wb.parent {
            children.entry(parent).or_default().push(wb.id);
        }
    }
    for kids in children.values_mut() {
        kids.sort_by(|&a, &b| {
            let sa = model.work_blocks.get(&a).map(|w| w.start_day).unwrap_or(0);
            let sb = model.work_blocks.get(&b).map(|w| w.start_day).unwrap_or(0);
            sa.cmp(&sb).then(a.0.cmp(&b.0))
        });
    }
    let mut result = Vec::new();
    let mut stack: Vec<WorkBlockId> = plan.root_blocks.iter().rev().cloned().collect();
    while let Some(id) = stack.pop() {
        result.push(id);
        if let Some(kids) = children.get(&id) {
            for &kid in kids.iter().rev() {
                stack.push(kid);
            }
        }
    }
    result
}

/// Formats one RFC-4180 CSV row from a slice of field values.
fn csv_row(fields: &[&str]) -> String {
    let mut row = fields
        .iter()
        .map(|f| csv_field(f))
        .collect::<Vec<_>>()
        .join(",");
    row.push('\n');
    row
}

/// Quotes a single field if it contains commas, double-quotes, or newlines.
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ── Import ────────────────────────────────────────────────────────────────────

/// One parsed row from a blocks CSV.
#[derive(Debug, PartialEq)]
pub struct CsvRow {
    /// The `id` value from the CSV; used only as a cross-reference key for
    /// `parent_id`. New block IDs are allocated by the model on import.
    pub old_id: u64,
    pub name: String,
    pub parent_old_id: Option<u64>,
    pub start_date: chrono::NaiveDate,
    pub duration_days: i32,
    pub row: i32,
    pub row_name: String,
}

/// Parses a blocks CSV string into validated rows.
///
/// Returns `Ok(rows)` when all rows parse cleanly.  Returns `Err(errors)` with
/// one human-readable message per bad line; good lines are still included in the
/// partial result via the tuple but the caller should treat the whole import as
/// failed when errors is non-empty.
pub fn parse_csv(csv: &str) -> Result<Vec<CsvRow>, Vec<String>> {
    let mut rows = Vec::new();
    let mut errors = Vec::new();

    // The header is always a single physical line. The data rows below it are
    // full RFC-4180 records, which may span several physical lines when a quoted
    // field carries an embedded newline — the inverse of `csv_field`'s quoting.
    let (header_line, body) = csv.split_once('\n').unwrap_or((csv, ""));
    let header = header_line.trim();
    if header != "id,name,parent_id,start_date,duration_days,row,row_name" {
        errors.push(format!(
            "Line 1: expected header \
             'id,name,parent_id,start_date,duration_days,row,row_name', got '{header}'"
        ));
        return Err(errors);
    }

    for (idx, fields) in parse_csv_records(body).into_iter().enumerate() {
        let line_num = idx + 2;
        if fields.len() != 7 {
            errors.push(format!(
                "Line {line_num}: expected 7 fields, got {}",
                fields.len()
            ));
            continue;
        }

        macro_rules! bad {
            ($msg:expr) => {{
                errors.push(format!("Line {line_num}: {}", $msg));
                continue;
            }};
        }

        let old_id = match fields[0].parse::<u64>() {
            Ok(v) => v,
            Err(_) => bad!(format!("invalid id {:?}", fields[0])),
        };
        let name = fields[1].clone();
        if name.is_empty() {
            bad!("name must not be empty");
        }
        let parent_old_id = if fields[2].is_empty() {
            None
        } else {
            match fields[2].parse::<u64>() {
                Ok(v) => Some(v),
                Err(_) => bad!(format!("invalid parent_id {:?}", fields[2])),
            }
        };
        let start_date = match chrono::NaiveDate::parse_from_str(&fields[3], "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => bad!(format!("invalid start_date {:?}", fields[3])),
        };
        let duration_days = match fields[4].parse::<i32>() {
            Ok(v) if v >= 1 => v,
            Ok(v) => bad!(format!("duration_days must be >= 1, got {v}")),
            Err(_) => bad!(format!("invalid duration_days {:?}", fields[4])),
        };
        let row = match fields[5].parse::<i32>() {
            Ok(v) if v >= 0 => v,
            Ok(v) => bad!(format!("row must be >= 0, got {v}")),
            Err(_) => bad!(format!("invalid row {:?}", fields[5])),
        };
        let row_name = fields[6].clone();

        rows.push(CsvRow {
            old_id,
            name,
            parent_old_id,
            start_date,
            duration_days,
            row,
            row_name,
        });
    }

    if errors.is_empty() {
        Ok(rows)
    } else {
        Err(errors)
    }
}

/// Populates `plan_id` with blocks from `rows`, converting dates to working-day
/// offsets via `model.calendar`.  Returns a list of errors (empty on success).
///
/// Rows must arrive in DFS pre-order (parent before child) — this is the order
/// `plan_to_csv` always writes.  A row whose `parent_old_id` has not yet been
/// seen produces an error for that row (and its subtree will also fail).
///
/// Call `clear_plan_blocks` first when replacing an existing plan's content.
pub fn populate_plan(rows: &[CsvRow], plan_id: PlanId, model: &mut Model) -> Vec<String> {
    let known_ids: std::collections::HashSet<u64> = rows.iter().map(|r| r.old_id).collect();
    let mut errors = Vec::new();

    // Pre-validate: every parent_old_id must exist somewhere in the file.
    for (idx, row) in rows.iter().enumerate() {
        if let Some(pid) = row.parent_old_id {
            if !known_ids.contains(&pid) {
                errors.push(format!(
                    "Row {}: parent_id {pid} not found in this file",
                    idx + 2
                ));
            }
        }
    }
    if !errors.is_empty() {
        return errors;
    }

    // Map: old_id (from CSV) → new WorkBlockId allocated in this model.
    let mut id_map: HashMap<u64, WorkBlockId> = HashMap::new();
    // Accumulate row names to set after all blocks are created.
    let mut row_names: HashMap<i32, String> = HashMap::new();

    for (idx, row) in rows.iter().enumerate() {
        let start_day = calendar::date_to_day(row.start_date, &model.calendar);

        let new_id = if let Some(parent_old_id) = row.parent_old_id {
            match id_map.get(&parent_old_id) {
                Some(&parent_new_id) => model.add_child_block(
                    plan_id,
                    parent_new_id,
                    &row.name,
                    start_day,
                    row.duration_days,
                    row.row,
                ),
                None => {
                    errors.push(format!(
                        "Row {}: parent_id {} appears after child in file (must be pre-order)",
                        idx + 2,
                        parent_old_id
                    ));
                    continue;
                }
            }
        } else {
            model.add_block_to_plan(plan_id, &row.name, start_day, row.duration_days, row.row)
        };

        id_map.insert(row.old_id, new_id);

        if !row.row_name.is_empty() {
            row_names
                .entry(row.row)
                .or_insert_with(|| row.row_name.clone());
        }
    }

    // Apply row names to the plan.
    if let Some(plan) = model.plans.get_mut(&plan_id) {
        for (row, name) in row_names {
            plan.set_row_name(None, row, name);
        }
    }

    errors
}

/// Removes all blocks (roots and children) from `plan_id` and discards them
/// from the model if no other plan still references them.  Clears row names too.
///
/// Used by the "replace existing plan" import mode before repopulating.
pub fn clear_plan_blocks(model: &mut Model, plan_id: PlanId) {
    // Collect all block IDs that have a row assignment in this plan.
    // `block_rows` holds both roots and children.
    let plan_block_ids: Vec<WorkBlockId> = model
        .plans
        .get(&plan_id)
        .map(|p| p.block_rows.keys().cloned().collect())
        .unwrap_or_default();

    if let Some(plan) = model.plans.get_mut(&plan_id) {
        plan.root_blocks.clear();
        plan.block_rows.clear();
        plan.row_names.clear();
    }

    for block_id in plan_block_ids {
        let still_referenced = model
            .plans
            .values()
            .any(|p| p.block_rows.contains_key(&block_id));
        if !still_referenced {
            model.work_blocks.remove(&block_id);
            model
                .dependencies
                .retain(|_, d| d.predecessor != block_id && d.successor != block_id);
        }
    }
}

/// Splits an RFC-4180 CSV body into records, each a vector of fields.
///
/// A quoted field (`"…"`) may contain commas, doubled-quote escapes (`""` → a
/// literal `"`), and embedded newlines; record boundaries are the newlines that
/// fall *outside* quotes. This is the exact inverse of the quoting `csv_field`
/// applies on export, so a multi-line quoted field round-trips. Both `\n` and
/// `\r\n` line endings are accepted; a trailing newline does not yield an empty
/// trailing record.
fn parse_csv_records(body: &str) -> Vec<Vec<String>> {
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = body.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => in_quotes = false,
                other => field.push(other),
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => record.push(std::mem::take(&mut field)),
                // `\r\n`: drop the `\r` and let the `\n` end the record. A lone
                // `\r` (rare) also ends the record.
                '\r' if chars.peek() == Some(&'\n') => {}
                '\n' | '\r' => {
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                }
                other => field.push(other),
            }
        }
    }
    // Flush a final record that ended without a trailing newline.
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CalendarConfig, Model};
    use chrono::NaiveDate;

    fn base_model() -> Model {
        let mut m = Model::default();
        m.calendar = CalendarConfig {
            start_date: NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(),
            working_days_per_week: 5,
            non_working_dates: vec![],
            quarter_colors: [[0.0; 4]; 4],
        };
        m.create_plan("main", None);
        m
    }

    #[test]
    fn csv_field_plain_passthrough() {
        assert_eq!(csv_field("hello"), "hello");
        assert_eq!(csv_field(""), "");
    }

    #[test]
    fn csv_field_quotes_commas() {
        assert_eq!(csv_field("a,b"), "\"a,b\"");
    }

    #[test]
    fn csv_field_doubles_internal_quotes() {
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn csv_field_quotes_newline() {
        assert_eq!(csv_field("line\nbreak"), "\"line\nbreak\"");
    }

    #[test]
    fn empty_plan_produces_only_header() {
        let m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        let plan = &m.plans[&plan_id];
        let csv = plan_to_csv(plan, &m);
        assert_eq!(csv, HEADER);
    }

    #[test]
    fn single_root_block_round_trip() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        let block_id = m.add_block_to_plan(plan_id, "Alpha", 0, 5, 0);

        let plan = &m.plans[&plan_id];
        let csv = plan_to_csv(plan, &m);

        let mut lines = csv.lines();
        assert_eq!(
            lines.next().unwrap(),
            "id,name,parent_id,start_date,duration_days,row,row_name"
        );
        let row = lines.next().unwrap();
        assert!(row.contains(&block_id.0.to_string()), "id present");
        assert!(row.contains("Alpha"), "name present");
        assert!(
            row.contains("2025-01-06"),
            "start_date is day 0 = 2025-01-06"
        );
        assert!(row.contains(",5,"), "duration_days = 5");
        // parent_id is empty (root)
        let fields: Vec<&str> = row.split(',').collect();
        assert_eq!(fields[2], "", "parent_id empty for root block");
    }

    #[test]
    fn child_block_parent_id_matches_parent() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        let parent_id = m.add_block_to_plan(plan_id, "Parent", 0, 10, 0);
        let child_id = m.add_child_block(plan_id, parent_id, "Child", 0, 5, 1);

        let plan = &m.plans[&plan_id];
        let csv = plan_to_csv(plan, &m);

        // Find the child row by its name.
        let child_row = csv
            .lines()
            .find(|l| l.contains("Child"))
            .expect("child row present");
        let fields: Vec<&str> = child_row.split(',').collect();
        assert_eq!(
            fields[0],
            child_id.0.to_string(),
            "child id in first column"
        );
        assert_eq!(
            fields[2],
            parent_id.0.to_string(),
            "parent_id column matches parent"
        );
    }

    #[test]
    fn preorder_puts_parent_before_child() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        let parent_id = m.add_block_to_plan(plan_id, "Parent", 0, 10, 0);
        m.add_child_block(plan_id, parent_id, "Child", 2, 5, 0);

        let plan = &m.plans[&plan_id];
        let csv = plan_to_csv(plan, &m);

        let lines: Vec<&str> = csv.lines().collect();
        let parent_pos = lines.iter().position(|l| l.contains("Parent")).unwrap();
        let child_pos = lines.iter().position(|l| l.contains("Child")).unwrap();
        assert!(parent_pos < child_pos, "parent row must precede child row");
    }

    #[test]
    fn block_name_with_comma_is_quoted() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        m.add_block_to_plan(plan_id, "Fee, Fi, Fo", 0, 3, 0);

        let plan = &m.plans[&plan_id];
        let csv = plan_to_csv(plan, &m);

        assert!(
            csv.contains("\"Fee, Fi, Fo\""),
            "name with commas must be quoted"
        );
    }

    #[test]
    fn row_name_appears_in_last_column() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        m.add_block_to_plan(plan_id, "Task", 0, 5, 0);
        // Name row 0 for the main plan scope.
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .set_row_name(None, 0, "Alice".to_string());

        let plan = &m.plans[&plan_id];
        let csv = plan_to_csv(plan, &m);

        let data_row = csv.lines().nth(1).unwrap();
        let fields: Vec<&str> = data_row.split(',').collect();
        assert_eq!(fields.last().copied().unwrap_or(""), "Alice");
    }

    #[test]
    fn start_date_advances_by_working_days() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        // Day 5 = Monday 2025-01-13 (skip weekend).
        m.add_block_to_plan(plan_id, "Week2", 5, 3, 0);

        let plan = &m.plans[&plan_id];
        let csv = plan_to_csv(plan, &m);

        assert!(
            csv.contains("2025-01-13"),
            "start_date at day 5 should be 2025-01-13"
        );
    }

    #[test]
    fn multiple_root_blocks_all_appear() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        m.add_block_to_plan(plan_id, "Alpha", 0, 3, 0);
        m.add_block_to_plan(plan_id, "Beta", 3, 3, 0);
        m.add_block_to_plan(plan_id, "Gamma", 6, 3, 0);

        let plan = &m.plans[&plan_id];
        let csv = plan_to_csv(plan, &m);

        assert!(csv.contains("Alpha"));
        assert!(csv.contains("Beta"));
        assert!(csv.contains("Gamma"));
        // Header + 3 data rows.
        assert_eq!(csv.lines().count(), 4);
    }

    // ── Import tests ──────────────────────────────────────────────────────────

    #[test]
    fn parse_csv_rejects_wrong_header() {
        let err = parse_csv("wrong,header\n1,Foo,,2025-01-06,5,0,\n").unwrap_err();
        assert!(!err.is_empty());
        assert!(err[0].contains("Line 1"));
    }

    #[test]
    fn parse_csv_rejects_wrong_field_count() {
        let csv = "id,name,parent_id,start_date,duration_days,row,row_name\n1,Foo\n";
        let err = parse_csv(csv).unwrap_err();
        assert!(err[0].contains("Line 2"));
        assert!(err[0].contains("fields"));
    }

    #[test]
    fn parse_csv_rejects_empty_name() {
        let csv = "id,name,parent_id,start_date,duration_days,row,row_name\n1,,, 2025-01-06,5,0,\n";
        let err = parse_csv(csv).unwrap_err();
        assert!(err[0].contains("name"));
    }

    #[test]
    fn parse_csv_rejects_bad_date() {
        let csv = "id,name,parent_id,start_date,duration_days,row,row_name\n1,X,,not-a-date,5,0,\n";
        let err = parse_csv(csv).unwrap_err();
        assert!(err[0].contains("start_date"));
    }

    #[test]
    fn parse_csv_rejects_zero_duration() {
        let csv = "id,name,parent_id,start_date,duration_days,row,row_name\n1,X,,2025-01-06,0,0,\n";
        let err = parse_csv(csv).unwrap_err();
        assert!(err[0].contains("duration_days"));
    }

    #[test]
    fn parse_csv_accepts_quoted_name_with_comma() {
        let csv = "id,name,parent_id,start_date,duration_days,row,row_name\n1,\"Fee, Fi\",,2025-01-06,3,0,\n";
        let rows = parse_csv(csv).unwrap();
        assert_eq!(rows[0].name, "Fee, Fi");
    }

    #[test]
    fn parse_csv_parses_parent_id() {
        let csv = "id,name,parent_id,start_date,duration_days,row,row_name\n\
                   1,Parent,,2025-01-06,10,0,\n\
                   2,Child,1,2025-01-06,5,0,\n";
        let rows = parse_csv(csv).unwrap();
        assert_eq!(rows[0].parent_old_id, None);
        assert_eq!(rows[1].parent_old_id, Some(1));
    }

    #[test]
    fn parse_csv_records_handles_empty_trailing_field() {
        // Last field (row_name) may be empty — must still produce 7 fields.
        let records = parse_csv_records("1,Alpha,,2025-01-06,5,0,");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].len(), 7);
        assert_eq!(records[0][6], "");
    }

    #[test]
    fn parse_csv_records_unquotes_doubled_quotes() {
        let records = parse_csv_records("1,\"say \"\"hi\"\"\",, 2025-01-06,5,0,");
        assert_eq!(records[0][1], "say \"hi\"");
    }

    #[test]
    fn parse_csv_records_no_trailing_empty_record() {
        let records = parse_csv_records("1,A,,2025-01-06,5,0,\n2,B,,2025-01-06,5,0,\n");
        assert_eq!(
            records.len(),
            2,
            "a trailing newline must not add an empty record"
        );
    }

    #[test]
    fn parse_csv_records_reassembles_quoted_embedded_newline() {
        // A quoted field containing a newline is one field of one record — the
        // physical line break inside the quotes must not split the record.
        let records = parse_csv_records("1,\"line\nbreak\",,2025-01-06,5,0,\n");
        assert_eq!(records.len(), 1, "embedded newline stays within one record");
        assert_eq!(records[0].len(), 7);
        assert_eq!(records[0][1], "line\nbreak");
    }

    #[test]
    fn parse_csv_records_handles_crlf_line_endings() {
        let records = parse_csv_records("1,A,,2025-01-06,5,0,\r\n2,B,,2025-01-06,5,0,\r\n");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0][1], "A");
        assert_eq!(records[1][1], "B");
    }

    #[test]
    fn populate_plan_creates_root_blocks() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        let rows = parse_csv(
            "id,name,parent_id,start_date,duration_days,row,row_name\n\
             1,Alpha,,2025-01-06,5,0,\n\
             2,Beta,,2025-01-13,3,1,Alice\n",
        )
        .unwrap();
        let errors = populate_plan(&rows, plan_id, &mut m);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(m.plans[&plan_id].root_blocks.len(), 2);
        let names: Vec<_> = m.plans[&plan_id]
            .root_blocks
            .iter()
            .map(|id| m.work_blocks[id].name.clone())
            .collect();
        assert!(names.contains(&"Alpha".to_string()));
        assert!(names.contains(&"Beta".to_string()));
    }

    #[test]
    fn populate_plan_reconstructs_hierarchy() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        let rows = parse_csv(
            "id,name,parent_id,start_date,duration_days,row,row_name\n\
             10,Parent,,2025-01-06,10,0,\n\
             20,Child,10,2025-01-06,5,0,\n",
        )
        .unwrap();
        let errors = populate_plan(&rows, plan_id, &mut m);
        assert!(errors.is_empty(), "{errors:?}");
        // Child must have a parent pointer.
        let child = m
            .work_blocks
            .values()
            .find(|wb| wb.name == "Child")
            .unwrap();
        let parent = m
            .work_blocks
            .values()
            .find(|wb| wb.name == "Parent")
            .unwrap();
        assert_eq!(child.parent, Some(parent.id));
    }

    #[test]
    fn populate_plan_errors_on_unknown_parent_id() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        let rows = parse_csv(
            "id,name,parent_id,start_date,duration_days,row,row_name\n\
             1,Child,999,2025-01-06,5,0,\n",
        )
        .unwrap();
        let errors = populate_plan(&rows, plan_id, &mut m);
        assert!(!errors.is_empty());
        assert!(errors[0].contains("999"));
    }

    #[test]
    fn populate_plan_sets_row_names() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        let rows = parse_csv(
            "id,name,parent_id,start_date,duration_days,row,row_name\n\
             1,Task,,2025-01-06,5,2,Engineering\n",
        )
        .unwrap();
        let errors = populate_plan(&rows, plan_id, &mut m);
        assert!(errors.is_empty());
        assert_eq!(m.plans[&plan_id].row_name(None, 2), Some("Engineering"));
    }

    #[test]
    fn clear_plan_blocks_removes_all_blocks() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        let parent = m.add_block_to_plan(plan_id, "Parent", 0, 10, 0);
        m.add_child_block(plan_id, parent, "Child", 0, 5, 1);
        assert!(!m.plans[&plan_id].root_blocks.is_empty());

        clear_plan_blocks(&mut m, plan_id);

        assert!(m.plans[&plan_id].root_blocks.is_empty());
        assert!(m.plans[&plan_id].block_rows.is_empty());
        assert!(m.work_blocks.is_empty());
    }

    #[test]
    fn clear_plan_blocks_preserves_blocks_shared_with_other_plans() {
        let mut m = base_model();
        let main_id = m.main_plan_id().unwrap();
        let branch_id = m.create_plan("branch", Some(0));
        let block = m.add_block_to_plan(main_id, "Shared", 0, 5, 0);
        // Manually add to branch too.
        m.plans.get_mut(&branch_id).unwrap().root_blocks.push(block);
        m.plans
            .get_mut(&branch_id)
            .unwrap()
            .block_rows
            .insert(block, 0);

        clear_plan_blocks(&mut m, main_id);

        // Block is gone from main but still exists for branch.
        assert!(!m.plans[&main_id].block_rows.contains_key(&block));
        assert!(m.work_blocks.contains_key(&block));
    }

    #[test]
    fn export_import_round_trip() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        let parent = m.add_block_to_plan(plan_id, "Parent", 0, 10, 0);
        m.add_child_block(plan_id, parent, "Child A", 0, 3, 1);
        m.add_child_block(plan_id, parent, "Child B", 3, 5, 1);
        m.add_block_to_plan(plan_id, "Solo", 10, 2, 0);
        m.plans
            .get_mut(&plan_id)
            .unwrap()
            .set_row_name(None, 1, "Team".to_string());

        // Export to CSV.
        let csv = plan_to_csv(&m.plans[&plan_id], &m);

        // Import into a fresh plan on the same model.
        let import_id = m.create_plan("imported", None);
        let rows = parse_csv(&csv).unwrap();
        let errors = populate_plan(&rows, import_id, &mut m);
        assert!(errors.is_empty(), "import errors: {errors:?}");

        // Compare content of exported plan vs imported plan.
        let orig_blocks: Vec<_> = {
            let plan = &m.plans[&plan_id];
            blocks_preorder(plan, &m)
                .into_iter()
                .map(|id| {
                    let wb = &m.work_blocks[&id];
                    (
                        wb.name.clone(),
                        wb.start_day,
                        wb.duration_days,
                        plan.block_rows.get(&id).copied().unwrap_or(0),
                        wb.parent.map(|p| m.work_blocks[&p].name.clone()),
                    )
                })
                .collect()
        };
        let imp_blocks: Vec<_> = {
            let plan = &m.plans[&import_id];
            blocks_preorder(plan, &m)
                .into_iter()
                .map(|id| {
                    let wb = &m.work_blocks[&id];
                    (
                        wb.name.clone(),
                        wb.start_day,
                        wb.duration_days,
                        plan.block_rows.get(&id).copied().unwrap_or(0),
                        wb.parent.map(|p| m.work_blocks[&p].name.clone()),
                    )
                })
                .collect()
        };
        assert_eq!(orig_blocks, imp_blocks, "round-trip must be content-equal");

        // Row name must also round-trip.
        assert_eq!(
            m.plans[&import_id].row_name(None, 1),
            Some("Team"),
            "row name must survive round-trip"
        );
    }

    #[test]
    fn export_import_round_trips_name_with_embedded_newline() {
        // The asymmetry br-234 fixes: csv_field quotes embedded newlines on
        // export, so the importer must reassemble the quoted multi-line field
        // rather than fail the 7-field check on the split physical lines.
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        m.add_block_to_plan(plan_id, "two\nlines", 0, 5, 0);

        let csv = plan_to_csv(&m.plans[&plan_id], &m);
        // Precondition: export really did quote the newline into the field.
        assert!(
            csv.contains("\"two\nlines\""),
            "export should RFC-4180-quote the embedded newline"
        );

        let import_id = m.create_plan("imported", None);
        let rows = parse_csv(&csv).expect("import must succeed, not fail the field count");
        let errors = populate_plan(&rows, import_id, &mut m);
        assert!(errors.is_empty(), "{errors:?}");

        let name = m.plans[&import_id]
            .root_blocks
            .first()
            .map(|id| m.work_blocks[id].name.clone())
            .expect("one imported block");
        assert_eq!(
            name, "two\nlines",
            "the embedded newline survives the round trip"
        );
    }

    #[test]
    fn replace_plan_clears_then_repopulates() {
        let mut m = base_model();
        let plan_id = m.main_plan_id().unwrap();
        m.add_block_to_plan(plan_id, "OldBlock", 0, 5, 0);
        assert_eq!(m.plans[&plan_id].root_blocks.len(), 1);

        let csv = "id,name,parent_id,start_date,duration_days,row,row_name\n\
                   1,NewBlock,,2025-01-06,3,0,\n\
                   2,Another,,2025-01-13,2,0,\n";
        clear_plan_blocks(&mut m, plan_id);
        let rows = parse_csv(csv).unwrap();
        let errors = populate_plan(&rows, plan_id, &mut m);
        assert!(errors.is_empty());

        assert_eq!(m.plans[&plan_id].root_blocks.len(), 2);
        let names: Vec<_> = m.work_blocks.values().map(|wb| wb.name.clone()).collect();
        assert!(
            !names.contains(&"OldBlock".to_string()),
            "old block removed"
        );
        assert!(names.contains(&"NewBlock".to_string()));
        assert!(names.contains(&"Another".to_string()));
    }
}
