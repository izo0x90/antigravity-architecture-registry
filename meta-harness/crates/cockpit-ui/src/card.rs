//! A component's card, as every view shows it: its sections, in order, each
//! with the stable name notes use for it and its title.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Overview,
    Signature,
    Logic,
    Invariants,
    Relations,
    Code,
    Changes,
    Placeholders,
    Findings,
}

impl Section {
    pub const ALL: [Section; 9] = [
        Section::Overview,
        Section::Signature,
        Section::Logic,
        Section::Invariants,
        Section::Relations,
        Section::Code,
        Section::Changes,
        Section::Placeholders,
        Section::Findings,
    ];

    /// The stable name notes use for it (`harness_contracts::CARD_SECTIONS`).
    pub fn key(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Signature => "signature",
            Self::Logic => "logic",
            Self::Invariants => "invariants",
            Self::Relations => "relations",
            Self::Code => "code",
            Self::Changes => "changes",
            Self::Placeholders => "placeholders",
            Self::Findings => "findings",
        }
    }

    pub fn from_key(key: &str) -> Option<Section> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }

    /// Where a required detail is written.
    pub fn of_required(field: arch_graph::RequiredField) -> Section {
        use arch_graph::RequiredField as F;
        match field {
            F::Description => Self::Overview,
            F::Signature | F::Inputs | F::Outputs | F::Properties | F::SideEffects => Self::Signature,
            F::Spec | F::Pattern | F::LogicSteps | F::Validation => Self::Logic,
            F::Invariants => Self::Invariants,
        }
    }

    /// Where a field changed since the last review shows (diff field paths).
    pub fn of_field(field: &str) -> Section {
        match field {
            "signature" => Self::Signature,
            f if f.starts_with("schemas") => Self::Signature,
            f if f.starts_with("spec.logic_steps") || f.starts_with("spec.tasks") => Self::Logic,
            f if f.starts_with("spec.invariants") => Self::Invariants,
            "relations" => Self::Relations,
            "placeholders" => Self::Placeholders,
            _ => Self::Overview,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Signature => "Signature",
            Self::Logic => "Logic steps",
            Self::Invariants => "Invariants",
            Self::Relations => "Relations",
            Self::Code => "Code",
            Self::Changes => "Changes",
            Self::Placeholders => "Placeholders",
            Self::Findings => "Findings",
        }
    }
}
