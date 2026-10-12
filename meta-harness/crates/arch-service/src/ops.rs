//! Plan operations: the only way plans change. Each validates its inputs
//! against the code and the plan, and rejects what can't be right.

use arch_graph::{
    Component, ComponentId, Graph, ImplementationSpec, Plan, Provenance, ProposedLocation, Reference, Relation, RelationKind, RepoPath,
    Schemas, Signature, Stage, Status, TypeSystem,
    schema::Dsl,
};
use code_analyzer::{PackSet, module_symbol};
use serde::{Deserialize, Serialize};

use crate::ServiceError;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanOp {
    /// A new component, placed under `parent` (and in its file) or at the top
    /// of `file`.
    AddComponent {
        name: String,
        #[serde(rename = "type")]
        type_name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        parent: Option<ComponentId>,
        #[serde(default)]
        file: Option<String>,
        #[serde(default)]
        signature: Option<Signature>,
        /// Data shapes, in the schema DSL or as JSON Schema.
        #[serde(default)]
        schemas: Option<Schemas>,
        #[serde(default)]
        spec: Option<ImplementationSpec>,
    },
    /// Edit a component this plan has, or plan changes to one in the code.
    Modify {
        id: ComponentId,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        signature: Option<Signature>,
        /// Data shapes, in the schema DSL or as JSON Schema.
        #[serde(default)]
        schemas: Option<Schemas>,
        #[serde(default)]
        spec: Option<ImplementationSpec>,
    },
    /// Build on an existing component (and its neighborhood by kinds/depth).
    Reference {
        id: ComponentId,
        #[serde(default)]
        kinds: Vec<RelationKind>,
        #[serde(default)]
        depth: u32,
    },
    Relate {
        kind: RelationKind,
        source: ComponentId,
        target: ComponentId,
        #[serde(default)]
        order: Option<u32>,
        #[serde(default)]
        ports: Option<(String, String)>,
        #[serde(default)]
        note: Option<String>,
        /// Calls: what the caller passes, gets back and expects changed
        /// (schema DSL or JSON Schema).
        #[serde(default)]
        expects: Option<Schemas>,
    },
    Unrelate { kind: RelationKind, source: ComponentId, target: ComponentId },
    /// Remove a component this plan added (or its planned changes).
    Remove { id: ComponentId },
    SetGreenfield { greenfield: bool },
    Describe {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        description: Option<String>,
    },
}

impl PlanOp {
    /// The component whose design this op changes (what depends on it must
    /// be looked at again), if any.
    pub fn design_change(&self) -> Option<&ComponentId> {
        match self {
            PlanOp::Modify { id, .. } | PlanOp::Remove { id } => Some(id),
            PlanOp::Relate { kind, source, target, .. } | PlanOp::Unrelate { kind, source, target } => match kind {
                RelationKind::Calls | RelationKind::Implements => Some(source),
                RelationKind::DataFlow | RelationKind::Contains => Some(target),
            },
            PlanOp::AddComponent { .. } | PlanOp::Reference { .. } | PlanOp::SetGreenfield { .. } | PlanOp::Describe { .. } => None,
        }
    }
}

fn invalid(msg: impl Into<String>) -> ServiceError {
    ServiceError::InvalidOp(msg.into())
}

/// Applies `op` to `plan`. `graph` is the code merged with this plan.
/// Schemas are stored compiled: the DSL is only an input format.
fn compile(schemas: Schemas, dsl: &Dsl) -> Result<Schemas, ServiceError> {
    let one = |s: Option<serde_json::Value>, what: &str| s.map(|v| dsl.compile(&v).map_err(|e| invalid(format!("{what}: {e}")))).transpose();
    Ok(Schemas { properties: one(schemas.properties, "properties")?, inputs: one(schemas.inputs, "inputs")?, outputs: one(schemas.outputs, "outputs")?, side_effects: schemas.side_effects })
}

#[allow(clippy::too_many_arguments)]
pub fn apply(plan: &mut Plan, op: PlanOp, graph: &Graph, root: &std::path::Path, ignores: &code_analyzer::Ignores, packs: &PackSet, types: &TypeSystem, dsl: &Dsl) -> Result<Vec<ComponentId>, ServiceError> {
    match op {
        PlanOp::AddComponent { name, type_name, description, parent, file, signature, schemas, spec } => {
            let schemas = compile(schemas.unwrap_or_default(), dsl)?;
            if name.trim().is_empty() || name.contains(char::is_whitespace) {
                return Err(invalid(format!("component name '{name}' must be a single identifier")));
            }
            let resolved = types.resolve(&type_name).map_err(|e| invalid(e.to_string()))?;
            if signature.is_some() && !resolved.allows_signature {
                return Err(invalid(format!("type '{type_name}' doesn't take a signature")));
            }
            let (language, file, symbol, parent) = match parent {
                Some(parent_id) => {
                    let parent_c = graph.component(&parent_id).ok_or_else(|| invalid(format!("parent {parent_id} doesn't exist")))?;
                    if !types.may_contain(&parent_c.type_name, &type_name).map_err(|e| invalid(e.to_string()))? {
                        return Err(invalid(format!("a {} can't contain a {type_name}", parent_c.type_name)));
                    }
                    let (language, parent_symbol, parent_file) = match (&parent_c.anchor, &parent_c.proposed) {
                        (Some(a), _) => (a.language.clone(), a.symbol.clone(), a.file.clone()),
                        (None, Some(p)) => (p.language.clone(), p.symbol.clone(), p.file.clone()),
                        (None, None) => return Err(invalid(format!("parent {parent_id} has no location yet"))),
                    };
                    let pack = packs.by_name(&language).ok_or_else(|| invalid(format!("no language pack for {language}")))?;
                    let file = match file {
                        Some(f) => RepoPath::new(&f).map_err(|e| invalid(e.to_string()))?,
                        None => parent_file,
                    };
                    (language, file, format!("{parent_symbol}{}{name}", pack.spec.language.separator), Some(parent_id))
                }
                None => {
                    let file = file.ok_or_else(|| invalid("a component needs a parent or a file"))?;
                    let file = RepoPath::new(&file).map_err(|e| invalid(e.to_string()))?;
                    let pack = packs.for_path(std::path::Path::new(file.as_str())).ok_or_else(|| invalid(format!("no language pack handles {file}")))?;
                    let module = module_symbol(root, &file, pack);
                    let parent = Some(ComponentId::for_symbol(pack.name(), &module)).filter(|m| graph.component(m).is_some());
                    (pack.name().to_owned(), file, format!("{module}{}{name}", pack.spec.language.separator), parent)
                }
            };
            if let Some(pack) = packs.by_name(&language) {
                pack.spec.language.check_name(&name).map_err(|e| invalid(format!("component name: {e}")))?;
            }
            if ignores.is_ignored(&root.join(file.as_str())) {
                return Err(invalid(format!("{file} is git-ignored or hidden: ingestion never reads it, so code there could never be verified")));
            }
            let id = ComponentId::for_symbol(&language, &symbol);
            if graph.component(&id).is_some() {
                return Err(invalid(format!("{id} already exists; modify it instead, or pick another name")));
            }
            plan.components.push(Component {
                id: id.clone(),
                name,
                type_name,
                description,
                status: Status::New,
                stage: Stage::Declared,
                anchor: None,
                proposed: Some(ProposedLocation { language, file, parent: parent.clone(), symbol }),
                signature,
                schemas,
                spec,
                placeholders: Vec::new(),
            });
            if let Some(parent) = parent {
                plan.relations.push(Relation::new(RelationKind::Contains, parent, id.clone(), Provenance::PLANNED));
            }
            Ok(vec![id])
        }
        PlanOp::Modify { id, description, signature, schemas, spec } => {
            let schemas = schemas.map(|s| compile(s, dsl)).transpose()?;
            let code = graph.component(&id).ok_or_else(|| invalid(format!("{id} doesn't exist")))?;
            // Anything this plan already has can be edited; otherwise only
            // code can be brought in for changes.
            if plan.component(&id).is_none() && code.anchor.is_none() {
                return Err(invalid(format!("{id} isn't in the code or in this plan")));
            }
            if plan.component(&id).is_none() {
                let mut planned = code.clone();
                planned.status = if code.status == Status::New { Status::New } else { Status::Modifying };
                planned.stage = Stage::Declared;
                planned.anchor = None;
                planned.placeholders.clear();
                plan.components.push(planned);
            }
            let planned = plan.component_mut(&id).ok_or_else(|| invalid(format!("{id} vanished")))?;
            if let Some(d) = description {
                planned.description = d;
            }
            if let Some(s) = signature {
                planned.signature = Some(s);
            }
            if let Some(s) = schemas {
                planned.schemas = s;
            }
            if let Some(s) = spec {
                planned.spec = Some(s);
            }
            Ok(vec![id])
        }
        PlanOp::Reference { id, kinds, depth } => {
            let code = graph.component(&id).ok_or_else(|| invalid(format!("{id} doesn't exist")))?;
            if code.anchor.is_none() {
                return Err(invalid(format!("{id} isn't in the code; only existing components can be referenced")));
            }
            plan.references.retain(|r| r.id != id);
            plan.references.push(Reference { id: id.clone(), kinds, depth });
            Ok(vec![id])
        }
        PlanOp::Relate { kind, source, target, order, ports, note, expects } => {
            for end in [&source, &target] {
                if graph.component(end).is_none() {
                    return Err(invalid(format!("{end} doesn't exist")));
                }
            }
            if source == target {
                return Err(invalid("a relation needs two different components"));
            }
            if kind == RelationKind::Calls {
                let caller = graph.component(&source).map(|c| c.type_name.clone()).unwrap_or_default();
                if !types.resolve(&caller).map(|t| t.executable).unwrap_or(false) {
                    return Err(invalid(format!("{source} is a {caller}, which can't call anything")));
                }
            }
            // Containment follows the type rules, and each component has one
            // container (the one add_component gave it, or the code's).
            if kind == RelationKind::Contains {
                let type_of = |id: &ComponentId| graph.component(id).map(|c| c.type_name.clone()).unwrap_or_default();
                let (parent, child) = (type_of(&source), type_of(&target));
                if !types.may_contain(&parent, &child).map_err(|e| invalid(e.to_string()))? {
                    return Err(invalid(format!("{source} is a {parent}, which can't contain {target} (a {child})")));
                }
                if let Some(existing) = graph.parent(&target).filter(|p| **p != source) {
                    return Err(invalid(format!("{target} is already inside {existing}; a component has one container")));
                }
            }
            if kind == RelationKind::DataFlow && ports.is_none() {
                return Err(invalid("a data flow needs ports (source output, target input)"));
            }
            let mut relation = Relation::new(kind, source.clone(), target.clone(), Provenance::PLANNED);
            relation.order = order;
            relation.ports = ports;
            relation.note = note.unwrap_or_default();
            if expects.is_some() && kind != RelationKind::Calls {
                return Err(invalid("only calls carry expectations"));
            }
            relation.expects = expects.map(|e| compile(e, dsl)).transpose()?;
            let rid = relation.id();
            plan.relations.retain(|r| r.id() != rid);
            plan.relations.push(relation);
            Ok(vec![source, target])
        }
        PlanOp::Unrelate { kind, source, target } => {
            let before = plan.relations.len();
            plan.relations.retain(|r| !(r.kind == kind && r.source == source && r.target == target));
            if plan.relations.len() == before {
                return Err(invalid(format!("the plan has no {} relation {source} -> {target}", kind.name())));
            }
            Ok(vec![source, target])
        }
        PlanOp::Remove { id } => {
            if plan.component(&id).is_none() {
                return Err(invalid(format!("{id} isn't part of this plan")));
            }
            plan.components.retain(|c| c.id != id);
            plan.relations.retain(|r| r.source != id && r.target != id);
            Ok(vec![id])
        }
        PlanOp::SetGreenfield { greenfield } => {
            plan.greenfield = greenfield;
            Ok(Vec::new())
        }
        PlanOp::Describe { title, description } => {
            if let Some(t) = title {
                plan.title = t;
            }
            if let Some(d) = description {
                plan.description = d;
            }
            Ok(Vec::new())
        }
    }
}
