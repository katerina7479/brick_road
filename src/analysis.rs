use crate::model::{Day, DependencyId, DependencyType, Model, WorkBlockId};

/// A single dependency whose constraint is not satisfied by the current
/// `WorkBlock` placements (`start_day` / `duration_days`).
#[derive(Debug, Clone, PartialEq)]
pub struct DependencyViolation {
    pub dependency_id: DependencyId,
    pub predecessor: WorkBlockId,
    pub successor: WorkBlockId,
    pub dependency_type: DependencyType,
    /// Days by which the constraint is violated (always > 0 when present).
    pub violation_days: Day,
}

/// Check every dependency in `model` against the current user-placed
/// `start_day` / `duration_days` on each `WorkBlock`.
///
/// Constraint semantics (P = predecessor, S = successor):
///   FS:  S.start ≥ P.end
///   SS:  S.start ≥ P.start
///   FF:  S.end   ≥ P.end
///   SF:  S.end   ≥ P.start
///
/// A violation occurs when the required bound exceeds the placed value;
/// `violation_days` is the magnitude of the shortfall.
pub fn analyze_dependencies(model: &Model) -> Vec<DependencyViolation> {
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

        let violation_days = match dep.dependency_type {
            DependencyType::FinishToStart => pred_end - succ.start_day,
            DependencyType::StartToStart => pred.start_day - succ.start_day,
            DependencyType::FinishToFinish => pred_end - succ_end,
            DependencyType::StartToFinish => pred.start_day - succ_end,
        };

        if violation_days > 0 {
            violations.push(DependencyViolation {
                dependency_id: dep_id,
                predecessor: dep.predecessor,
                successor: dep.successor,
                dependency_type: dep.dependency_type,
                violation_days,
            });
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Day, Model};

    fn placed(model: &mut Model, name: &str, start: Day, dur: Day) -> WorkBlockId {
        let id = model.create_work_block(name);
        let wb = model.work_blocks.get_mut(&id).unwrap();
        wb.start_day = start;
        wb.duration_days = dur;
        id
    }

    // ── analyze_dependencies tests ──────────────────────────────────────────

    #[test]
    fn fs_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5);
        let b = placed(&mut m, "B", 5, 3);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        assert!(analyze_dependencies(&m).is_empty());
    }

    #[test]
    fn fs_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5);
        let b = placed(&mut m, "B", 3, 3);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        let v = &analyze_dependencies(&m);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].predecessor, a);
        assert_eq!(v[0].successor, b);
        assert_eq!(v[0].violation_days, 2);
    }

    #[test]
    fn ss_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 2, 4);
        let b = placed(&mut m, "B", 2, 3);
        m.create_dependency(a, b, DependencyType::StartToStart);
        assert!(analyze_dependencies(&m).is_empty());
    }

    #[test]
    fn ss_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 3, 4);
        let b = placed(&mut m, "B", 2, 3);
        m.create_dependency(a, b, DependencyType::StartToStart);
        let v = &analyze_dependencies(&m);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].violation_days, 1);
    }

    #[test]
    fn ff_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5);
        let b = placed(&mut m, "B", 1, 4);
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        assert!(analyze_dependencies(&m).is_empty());
    }

    #[test]
    fn ff_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 6);
        let b = placed(&mut m, "B", 1, 4);
        m.create_dependency(a, b, DependencyType::FinishToFinish);
        let v = &analyze_dependencies(&m);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].violation_days, 1);
    }

    #[test]
    fn sf_no_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 4, 2);
        let b = placed(&mut m, "B", 0, 5);
        m.create_dependency(a, b, DependencyType::StartToFinish);
        assert!(analyze_dependencies(&m).is_empty());
    }

    #[test]
    fn sf_violation() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 5, 2);
        let b = placed(&mut m, "B", 0, 4);
        m.create_dependency(a, b, DependencyType::StartToFinish);
        let v = &analyze_dependencies(&m);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].violation_days, 1);
    }

    #[test]
    fn no_violations_clean_schedule() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 5);
        let b = placed(&mut m, "B", 5, 8);
        let c = placed(&mut m, "C", 13, 4);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.create_dependency(b, c, DependencyType::FinishToStart);
        assert!(analyze_dependencies(&m).is_empty());
    }

    #[test]
    fn missing_block_skipped() {
        let mut m = Model::default();
        let a = placed(&mut m, "A", 0, 3);
        let b = placed(&mut m, "B", 5, 2);
        m.create_dependency(a, b, DependencyType::FinishToStart);
        m.work_blocks.remove(&b);
        assert!(analyze_dependencies(&m).is_empty());
    }
}
