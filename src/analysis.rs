use std::collections::HashMap;

use bevy::prelude::Resource;

use crate::model::{DependencyId, DependencyType, Model, WorkBlockId};

/// A single dependency whose constraint is not satisfied by the current
/// `WorkBlock` placements (`start_day` / `duration_days`).
#[derive(Debug, Clone, PartialEq)]
pub struct DependencyViolation {
    pub dependency_id: DependencyId,
    pub predecessor: WorkBlockId,
    pub successor: WorkBlockId,
    pub dependency_type: DependencyType,
    pub lag: f32,
    /// Days by which the constraint is violated (always > 0 when present).
    pub violation_days: f32,
}

/// All analysis results computed from the current model/plan state.
#[derive(Debug, Clone, Default, PartialEq, Resource)]
pub struct ScheduleAnalysis {
    pub violations: Vec<DependencyViolation>,
    /// Zero-float blocks in topological order (from user placement backward pass).
    pub critical_path: Vec<WorkBlockId>,
    /// Total float per block (latest_finish − earliest_finish over user placement).
    pub float: HashMap<WorkBlockId, f32>,
}

/// Check every dependency in `model` against the current user-placed
/// `start_day` / `duration_days` on each `WorkBlock`.
///
/// Constraint semantics (P = predecessor, S = successor, lag in days):
///   FS:  S.start     ≥ P.end   + lag
///   SS:  S.start     ≥ P.start + lag
///   FF:  S.end       ≥ P.end   + lag
///   SF:  S.end       ≥ P.start + lag
///
/// A violation occurs when the required bound exceeds the placed value;
/// `violation_days` is the magnitude of the shortfall.
pub fn analyze_dependencies(model: &Model) -> ScheduleAnalysis {
    let mut violations = Vec::new();

    for (&dep_id, dep) in &model.dependencies {
        let Some(pred) = model.work_blocks.get(&dep.predecessor) else {
            continue;
        };
        let Some(succ) = model.work_blocks.get(&dep.successor) else {
            continue;
        };

        let pred_end = pred.start_day + pred.duration_days;
        let succ_end = succ.start_day + succ.duration_days;
        let lag = dep.lag;

        let violation_days = match dep.dependency_type {
            DependencyType::FinishToStart => pred_end + lag - succ.start_day,
            DependencyType::StartToStart => pred.start_day + lag - succ.start_day,
            DependencyType::FinishToFinish => pred_end + lag - succ_end,
            DependencyType::StartToFinish => pred.start_day + lag - succ_end,
        };

        if violation_days > 0.0 {
            violations.push(DependencyViolation {
                dependency_id: dep_id,
                predecessor: dep.predecessor,
                successor: dep.successor,
                dependency_type: dep.dependency_type,
                lag,
                violation_days,
            });
        }
    }

    ScheduleAnalysis { violations, critical_path: vec![], float: HashMap::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Estimate, Model};

    fn est(d: f32) -> Estimate {
        Estimate { most_likely: d, optimistic: d, pessimistic: d, confidence: 1.0 }
    }

    fn placed(model: &mut Model, name: &str, start: f32, dur: f32) -> WorkBlockId {
        let id = model.create_work_block(name, est(dur));
        let wb = model.work_blocks.get_mut(&id).unwrap();
        wb.start_day = start;
        wb.duration_days = dur;
        id
    }

    #[test]
    fn fs_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 5.0); // ends day 5
        let b = placed(&mut m, "B", 5.0, 3.0); // starts day 5
        m.create_dependency(a, b, DependencyType::FinishToStart);
        let analysis = analyze_dependencies(&m);
        assert!(analysis.violations.is_empty());
    }

    #[test]
    fn fs_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 5.0); // ends day 5
        let b = placed(&mut m, "B", 3.0, 3.0); // starts day 3 — too early
        m.create_dependency(a, b, DependencyType::FinishToStart);
        let analysis = analyze_dependencies(&m);
        assert_eq!(analysis.violations.len(), 1);
        let v = &analysis.violations[0];
        assert_eq!(v.predecessor, a);
        assert_eq!(v.successor, b);
        assert!((v.violation_days - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn fs_with_lag_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 3.0); // ends day 3; +2 lag → must start ≥ 5
        let b = placed(&mut m, "B", 4.0, 2.0); // starts day 4 — short by 1
        let dep_id = m.create_dependency(a, b, DependencyType::FinishToStart);
        m.dependencies.get_mut(&dep_id).unwrap().lag = 2.0;
        let analysis = analyze_dependencies(&m);
        assert_eq!(analysis.violations.len(), 1);
        assert!((analysis.violations[0].violation_days - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn ss_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 2.0, 4.0);
        let b = placed(&mut m, "B", 2.0, 3.0); // starts same day as A — OK
        m.create_dependency(a, b, DependencyType::StartToStart);
        assert!(analyze_dependencies(&m).violations.is_empty());
    }

    #[test]
    fn ss_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 3.0, 4.0);
        let b = placed(&mut m, "B", 2.0, 3.0); // B starts before A
        m.create_dependency(a, b, DependencyType::StartToStart);
        let v = &analyze_dependencies(&m).violations;
        assert_eq!(v.len(), 1);
        assert!((v[0].violation_days - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn ff_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 5.0); // ends day 5
        let b = placed(&mut m, "B", 1.0, 4.0); // ends day 5 — OK
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        assert!(analyze_dependencies(&m).violations.is_empty());
    }

    #[test]
    fn ff_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 6.0); // ends day 6
        let b = placed(&mut m, "B", 1.0, 4.0); // ends day 5 — short by 1
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        let v = &analyze_dependencies(&m).violations;
        assert_eq!(v.len(), 1);
        assert!((v[0].violation_days - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn sf_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 4.0, 2.0); // starts day 4
        let b = placed(&mut m, "B", 0.0, 5.0); // ends day 5 — OK (≥ 4)
        m.create_dependency(a, b, DependencyType::StartToFinish);
        assert!(analyze_dependencies(&m).violations.is_empty());
    }

    #[test]
    fn sf_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 5.0, 2.0); // starts day 5
        let b = placed(&mut m, "B", 0.0, 4.0); // ends day 4 — short by 1
        m.create_dependency(a, b, DependencyType::StartToFinish);
        let v = &analyze_dependencies(&m).violations;
        assert_eq!(v.len(), 1);
        assert!((v[0].violation_days - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn no_violations_clean_schedule() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 5.0);
        let b = placed(&mut m, "B", 5.0, 8.0);
        let c = placed(&mut m, "C", 13.0, 4.0);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        assert!(analyze_dependencies(&m).violations.is_empty());
    }

    #[test]
    fn missing_block_skipped() {
        // Dependency references an ID not in work_blocks — should not panic or report a violation.
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0.0, 3.0);
        let b = placed(&mut m, "B", 5.0, 2.0);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        // Manually remove one block to simulate a stale dependency.
        m.work_blocks.remove(&b);
        assert!(analyze_dependencies(&m).violations.is_empty());
    }
}
