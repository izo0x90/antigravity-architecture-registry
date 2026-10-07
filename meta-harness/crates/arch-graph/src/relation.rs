//! Relations: the edges of the graph, by kind, with where they came from.

use serde::{Deserialize, Serialize};

use crate::{ComponentId, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// Static structure: source contains target.
    Contains,
    /// Static structure: source implements or extends target.
    Implements,
    /// Dynamic: source calls target.
    Calls,
    /// Data moves from source to target.
    DataFlow,
}

impl RelationKind {
    pub const ALL: [RelationKind; 4] = [RelationKind::Contains, RelationKind::Implements, RelationKind::Calls, RelationKind::DataFlow];

    pub fn name(self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::Implements => "implements",
            Self::Calls => "calls",
            Self::DataFlow => "data_flow",
        }
    }
}

/// Where a relation is known from. Both set means the plan and the code agree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub planned: bool,
    pub observed: bool,
}

impl Provenance {
    pub const PLANNED: Provenance = Provenance { planned: true, observed: false };
    pub const OBSERVED: Provenance = Provenance { planned: false, observed: true };

    pub fn merge(self, other: Provenance) -> Provenance {
        Provenance { planned: self.planned || other.planned, observed: self.observed || other.observed }
    }

    pub fn is_both(self) -> bool {
        self.planned && self.observed
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelationId(pub String);

impl std::fmt::Display for RelationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub kind: RelationKind,
    pub source: ComponentId,
    pub target: ComponentId,
    #[serde(default)]
    pub provenance: Provenance,
    /// Calls: position among the caller's calls (0 = first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
    /// Calls: where in the caller's file the calls happen (observed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<Span>,
    /// DataFlow: the ports (output of source, input of target).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<(String, String)>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Calls: what the caller expects to pass (inputs), get back (outputs)
    /// and have changed (side effects), as JSON Schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expects: Option<crate::Schemas>,
    /// Calls: every observed site is deferred (inside a closure or similar),
    /// so the call may never run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deferred: bool,
    /// DataFlow: what flows (a type, a type's field, a boundary's key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

impl Relation {
    pub fn new(kind: RelationKind, source: ComponentId, target: ComponentId, provenance: Provenance) -> Self {
        Self { kind, source, target, provenance, order: None, sites: Vec::new(), ports: None, note: String::new(), expects: None, deferred: false, data: None }
    }

    /// One relation per kind and endpoint pair (data flows also per port pair).
    pub fn id(&self) -> RelationId {
        let base = format!("{}:{}->{}", self.kind.name(), self.source, self.target);
        match &self.ports {
            Some((from, to)) if self.kind == RelationKind::DataFlow => RelationId(format!("{base}:{from}->{to}")),
            _ => RelationId(base),
        }
    }

    /// Folds another observation/plan of the same relation into this one.
    pub fn absorb(&mut self, other: &Relation) {
        // Deferred only if every observation is.
        if other.provenance.observed {
            self.deferred = if self.provenance.observed { self.deferred && other.deferred } else { other.deferred };
        }
        self.provenance = self.provenance.merge(other.provenance);
        self.order = match (self.order, other.order) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        for site in &other.sites {
            if !self.sites.contains(site) {
                self.sites.push(*site);
            }
        }
        if self.note.is_empty() {
            self.note = other.note.clone();
        }
        if self.expects.is_none() {
            self.expects = other.expects.clone();
        }
        if self.data.is_none() {
            self.data = other.data.clone();
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    // req: P2.6
    #[test]
    fn relations_merge_provenance_and_sites() {
        let a = ComponentId::new("rust:app::run");
        let b = ComponentId::new("rust:app::step");
        let mut planned = Relation::new(RelationKind::Calls, a.clone(), b.clone(), Provenance::PLANNED);
        planned.order = Some(2);
        let mut observed = Relation::new(RelationKind::Calls, a.clone(), b.clone(), Provenance::OBSERVED);
        observed.order = Some(0);
        observed.sites.push(Span { start_line: 4, end_line: 4, start_byte: 40, end_byte: 50 });
        assert_eq!(planned.id(), observed.id());
        planned.absorb(&observed);
        assert!(planned.provenance.is_both());
        assert_eq!(planned.order, Some(0));
        assert_eq!(planned.sites.len(), 1);
        let contains = Relation::new(RelationKind::Contains, a.clone(), b.clone(), Provenance::OBSERVED);
        assert_ne!(contains.id(), planned.id(), "kind is part of identity");
        let mut flow = Relation::new(RelationKind::DataFlow, a, b, Provenance::PLANNED);
        flow.ports = Some(("out".into(), "input".into()));
        assert!(flow.id().0.ends_with(":out->input"));
        assert_eq!(serde_json::to_value(RelationKind::DataFlow).unwrap(), "data_flow");
    }
}
