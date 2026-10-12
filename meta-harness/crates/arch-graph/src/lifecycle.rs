//! Component status (where it comes from) and stage (how far along it is).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Planned, not in the code yet.
    New,
    /// Already in the code and not being changed.
    Existing,
    /// In the code, with planned changes.
    Modifying,
    /// In the code, planned for removal.
    Deprecated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Declared,
    ArchApproved,
    PlanApproved,
    /// Structure is in the code with tracked placeholder bodies.
    Skeleton,
    Implemented,
}

impl Stage {
    pub const ALL: [Stage; 5] = [Stage::Declared, Stage::ArchApproved, Stage::PlanApproved, Stage::Skeleton, Stage::Implemented];

    /// Stages from which the code is expected to contain the component.
    pub fn expects_code(self) -> bool {
        matches!(self, Stage::Skeleton | Stage::Implemented)
    }

    /// Stages that need a location (proposed or real).
    pub fn needs_location(self) -> bool {
        matches!(self, Stage::PlanApproved | Stage::Skeleton | Stage::Implemented)
    }
}

/// Allowed stage moves. Going back to Declared is always allowed (a change
/// re-opens the design); Skeleton can follow either approval.
pub fn can_transition(from: Stage, to: Stage) -> bool {
    use Stage::*;
    matches!(
        (from, to),
        (Declared, ArchApproved)
            | (ArchApproved, PlanApproved)
            | (ArchApproved, Skeleton)
            | (PlanApproved, Skeleton)
            | (PlanApproved, Implemented)
            | (Skeleton, PlanApproved)
            | (Skeleton, Implemented)
            | (_, Declared)
    ) && from != to
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_stage_use_snake_case_names_and_reject_unknowns() {
        assert_eq!(serde_json::to_string(&Stage::ArchApproved).unwrap(), "\"arch_approved\"");
        assert_eq!(serde_json::from_str::<Status>("\"modifying\"").unwrap(), Status::Modifying);
        assert!(serde_json::from_str::<Stage>("\"approved\"").is_err());
        assert!(serde_json::from_str::<Status>("\"New\"").is_err());
    }

    #[test]
    fn transition_table_allows_both_paths_into_skeleton() {
        use Stage::*;
        assert!(can_transition(ArchApproved, Skeleton), "structure first");
        assert!(can_transition(PlanApproved, Skeleton), "plan first");
        assert!(can_transition(Skeleton, PlanApproved), "plan after structure");
        assert!(can_transition(Skeleton, Implemented));
        assert!(!can_transition(Declared, Implemented));
        assert!(!can_transition(Declared, Skeleton));
        assert!(!can_transition(Implemented, Skeleton));
        assert!(!can_transition(Implemented, Implemented));
        assert!(can_transition(Implemented, Declared), "re-open");
    }
}
