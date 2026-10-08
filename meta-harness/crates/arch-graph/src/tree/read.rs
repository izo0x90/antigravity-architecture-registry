//! What building a tree reads from a graph. The in-memory [`Graph`] is the
//! one implementation; a graph too big for memory could implement this over
//! a database without anything that builds trees changing.

use crate::{Alignment, Component, ComponentId, Graph, Relation, RelationKind};

pub trait GraphRead {
    fn component(&self, id: &ComponentId) -> Option<&Component>;
    fn components(&self) -> Box<dyn Iterator<Item = &Component> + '_>;
    fn relations(&self) -> Box<dyn Iterator<Item = &Relation> + '_>;
    fn outgoing(&self, id: &ComponentId, kind: RelationKind) -> Box<dyn Iterator<Item = &Relation> + '_>;
    fn incoming(&self, id: &ComponentId, kind: RelationKind) -> Box<dyn Iterator<Item = &Relation> + '_>;
    fn parent(&self, id: &ComponentId) -> Option<&ComponentId>;
    fn alignment(&self, id: &ComponentId) -> Alignment;
    fn relation_alignment(&self, relation: &Relation) -> Alignment;
}

impl GraphRead for Graph {
    fn component(&self, id: &ComponentId) -> Option<&Component> {
        Graph::component(self, id)
    }
    fn components(&self) -> Box<dyn Iterator<Item = &Component> + '_> {
        Box::new(Graph::components(self))
    }
    fn relations(&self) -> Box<dyn Iterator<Item = &Relation> + '_> {
        Box::new(Graph::relations(self))
    }
    fn outgoing(&self, id: &ComponentId, kind: RelationKind) -> Box<dyn Iterator<Item = &Relation> + '_> {
        Box::new(Graph::outgoing(self, id, Some(kind)))
    }
    fn incoming(&self, id: &ComponentId, kind: RelationKind) -> Box<dyn Iterator<Item = &Relation> + '_> {
        Box::new(Graph::incoming(self, id, Some(kind)))
    }
    fn parent(&self, id: &ComponentId) -> Option<&ComponentId> {
        Graph::parent(self, id)
    }
    fn alignment(&self, id: &ComponentId) -> Alignment {
        Graph::alignment(self, id).unwrap_or(Alignment::Observed)
    }
    fn relation_alignment(&self, relation: &Relation) -> Alignment {
        Graph::relation_alignment(self, relation)
    }
}
