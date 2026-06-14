use std::collections::HashMap;

use crate::model::{PlanId, WorkBlockId};

/// The computed time placement of one work block.
#[derive(Debug, Clone)]
pub struct ScheduledBlock {
    pub work_block_id: WorkBlockId,
    pub start_day: f32,
    pub end_day: f32,
    /// Convenience: end_day - start_day.
    pub duration_days: f32,
}

/// The full output of a scheduler run over a Plan.
#[derive(Debug, Clone)]
pub struct Schedule {
    pub plan_id: PlanId,
    /// Placement for every block that was scheduled.
    pub blocks: HashMap<WorkBlockId, ScheduledBlock>,
    /// Day on which the last block finishes.
    pub total_duration_days: f32,
    /// Ordered sequence of block IDs on the critical path (longest path).
    pub critical_path: Vec<WorkBlockId>,
}

impl Schedule {
    pub fn new(plan_id: PlanId) -> Self {
        Self {
            plan_id,
            blocks: HashMap::new(),
            total_duration_days: 0.0,
            critical_path: vec![],
        }
    }
}
