macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(pub u64);
    };
}

id_newtype!(WorkBlockId);
id_newtype!(VariantId);
id_newtype!(ResourceBlockId);
id_newtype!(DependencyId);
id_newtype!(MilestoneId);
id_newtype!(WorldId);
id_newtype!(PlanId);

/// Three-point effort estimate in workdays.
#[derive(Debug, Clone)]
pub struct Estimate {
    pub most_likely: f32,
    pub optimistic: f32,
    pub pessimistic: f32,
    /// Subjective confidence that the true value falls in the given range (0.0–1.0).
    pub confidence: f32,
}

/// A unit of work. Leaf blocks carry their own estimate; blocks with variants
/// represent a choice between alternative implementations, each potentially
/// containing further child blocks.
#[derive(Debug, Clone)]
pub struct WorkBlock {
    pub id: WorkBlockId,
    pub name: String,
    /// Effort estimate for this block as a leaf. Ignored by the scheduler when
    /// `variants` is non-empty (rolled up from chosen variant's children instead).
    pub estimate: Estimate,
    /// Alternative implementations of this block (mutually exclusive).
    pub variants: Vec<VariantId>,
}

/// One alternative decomposition of a parent WorkBlock into an ordered sequence
/// of child WorkBlocks.
#[derive(Debug, Clone)]
pub struct Variant {
    pub id: VariantId,
    pub name: String,
    pub parent: WorkBlockId,
    /// Ordered child WorkBlocks that collectively implement this variant.
    pub children: Vec<WorkBlockId>,
}
