use std::collections::HashMap;

use crate::calendar;
use crate::model::{Model, Plan, WorkBlockId};

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
}
