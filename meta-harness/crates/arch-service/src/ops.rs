//! Plan operations: the only way plans change. Each validates its inputs
//! against the code and the plan, and rejects what can't be right.

use arch_graph::{
    Component, ComponentId, Graph, ImplementationSpec, Item, ItemId, ItemState, Plan, PlanKind, ProposedLocation, Provenance, Reference,
    Relation, RelationKind, RepoPath, Schemas, Signature, Stage, Status, TypeSystem, schema::Dsl,
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
    Unrelate {
        kind: RelationKind,
        source: ComponentId,
        target: ComponentId,
    },
    /// Remove a component this plan added (or its planned changes).
    Remove {
        id: ComponentId,
    },
    SetGreenfield {
        greenfield: bool,
    },
    Describe {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        description: Option<String>,
    },
    /// A new item (a finding, a decision, a to-do), under item `under` or
    /// at the top; open.
    AddItem {
        title: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        under: Option<ItemId>,
    },
    /// An item's title or text, rewritten.
    EditItem {
        id: ItemId,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        text: Option<String>,
    },
    /// An item moved under item `under` (none: to the top), last there.
    MoveItem {
        id: ItemId,
        #[serde(default)]
        under: Option<ItemId>,
    },
    /// Where an item stands now.
    SetItem {
        id: ItemId,
        state: ItemState,
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
            PlanOp::AddComponent { .. }
            | PlanOp::Reference { .. }
            | PlanOp::SetGreenfield { .. }
            | PlanOp::Describe { .. }
            | PlanOp::AddItem { .. }
            | PlanOp::EditItem { .. }
            | PlanOp::MoveItem { .. }
            | PlanOp::SetItem { .. } => None,
        }
    }

    /// Whether it's about the plan's items or its words (all a review plan
    /// takes).
    fn on_items(&self) -> bool {
        matches!(
            self,
            PlanOp::AddItem { .. } | PlanOp::EditItem { .. } | PlanOp::MoveItem { .. } | PlanOp::SetItem { .. } | PlanOp::Describe { .. }
        )
    }
}

impl PlanOp {
    /// What this op would do to the code, in words, if it changes it (a
    /// walkthrough explains the code and changes none of it).
    fn code_change(&self) -> Option<&'static str> {
        match self {
            PlanOp::AddComponent { .. } => Some("add components"),
            PlanOp::Modify { signature: Some(_), .. } => Some("change a signature"),
            PlanOp::Relate { .. } | PlanOp::Unrelate { .. } => Some("plan relations"),
            PlanOp::SetGreenfield { .. } => Some("be greenfield"),
            PlanOp::Modify { .. }
            | PlanOp::Reference { .. }
            | PlanOp::Remove { .. }
            | PlanOp::Describe { .. }
            | PlanOp::AddItem { .. }
            | PlanOp::EditItem { .. }
            | PlanOp::MoveItem { .. }
            | PlanOp::SetItem { .. } => None,
        }
    }
}

fn invalid(msg: impl Into<String>) -> ServiceError {
    ServiceError::InvalidOp(msg.into())
}

/// Applies `op` to `plan`. `graph` is the code merged with this plan.
/// Schemas are stored compiled: the DSL is only an input format.
fn compile(schemas: Schemas, dsl: &Dsl) -> Result<Schemas, ServiceError> {
    let one =
        |s: Option<serde_json::Value>, what: &str| s.map(|v| dsl.compile(&v).map_err(|e| invalid(format!("{what}: {e}")))).transpose();
    Ok(Schemas {
        properties: one(schemas.properties, "properties")?,
        inputs: one(schemas.inputs, "inputs")?,
        outputs: one(schemas.outputs, "outputs")?,
        side_effects: schemas.side_effects,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn apply(
    plan: &mut Plan,
    op: PlanOp,
    graph: &Graph,
    root: &std::path::Path,
    ignores: &code_analyzer::Ignores,
    packs: &PackSet,
    types: &TypeSystem,
    dsl: &Dsl,
) -> Result<Vec<ComponentId>, ServiceError> {
    if plan.kind == PlanKind::Walkthrough
        && let Some(what) = op.code_change()
    {
        return Err(invalid(format!("{} is a walkthrough: it explains the code as it is, so it can't {what}", plan.id)));
    }
    if plan.kind == PlanKind::Review && !op.on_items() {
        return Err(invalid(format!(
            "{} is a review: a list of items, with no components (add_item, edit_item, move_item, set_item)",
            plan.id
        )));
    }
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
                    let pack = packs
                        .for_path(std::path::Path::new(file.as_str()))
                        .ok_or_else(|| invalid(format!("no language pack handles {file}")))?;
                    let module = module_symbol(root, &file, pack);
                    let parent = Some(ComponentId::for_symbol(pack.name(), &module)).filter(|m| graph.component(m).is_some());
                    (pack.name().to_owned(), file, format!("{module}{}{name}", pack.spec.language.separator), parent)
                }
            };
            if let Some(pack) = packs.by_name(&language) {
                pack.spec.language.check_name(&name).map_err(|e| invalid(format!("component name: {e}")))?;
            }
            if ignores.is_ignored(&root.join(file.as_str())) {
                return Err(invalid(format!(
                    "{file} is git-ignored or hidden: ingestion never reads it, so code there could never be verified"
                )));
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
                // A walkthrough takes it as it is in the code; a change plans
                // a new version of it.
                if plan.kind == PlanKind::Change {
                    planned.status = if code.status == Status::New { Status::New } else { Status::Modifying };
                    planned.stage = Stage::Declared;
                    planned.anchor = None;
                    planned.placeholders.clear();
                }
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
        PlanOp::AddItem { title, text, under } => {
            let title = item_title(&title)?;
            if let Some(under) = &under {
                plan.item(under).ok_or_else(|| invalid(format!("{} has no item {under}", plan.id)))?;
            }
            let next = plan.items.iter().filter_map(|i| i.id.0.strip_prefix('i')?.parse::<u64>().ok()).max().unwrap_or(0) + 1;
            plan.items.push(Item { id: ItemId(format!("i{next}")), parent: under, title, text, state: ItemState::Open });
            Ok(Vec::new())
        }
        PlanOp::EditItem { id, title, text } => {
            let title = title.as_deref().map(item_title).transpose()?;
            let plan_id = plan.id.clone();
            let item = plan.items.iter_mut().find(|i| i.id == id).ok_or_else(|| invalid(format!("{plan_id} has no item {id}")))?;
            if let Some(title) = title {
                item.title = title;
            }
            if let Some(text) = text {
                item.text = text;
            }
            Ok(Vec::new())
        }
        PlanOp::MoveItem { id, under } => {
            let at = plan.items.iter().position(|i| i.id == id).ok_or_else(|| invalid(format!("{} has no item {id}", plan.id)))?;
            // Not under itself, nor under what's under it.
            let mut above = under.clone();
            while let Some(a) = above {
                if a == id {
                    return Err(invalid(format!(
                        "{id} can't go under {}: it's under {id}",
                        under.as_ref().map_or_else(String::new, ToString::to_string)
                    )));
                }
                above = plan.item(&a).ok_or_else(|| invalid(format!("{} has no item {a}", plan.id)))?.parent.clone();
            }
            let mut item = plan.items.remove(at);
            item.parent = under;
            plan.items.push(item);
            Ok(Vec::new())
        }
        PlanOp::SetItem { id, state } => {
            let plan_id = plan.id.clone();
            plan.items.iter_mut().find(|i| i.id == id).ok_or_else(|| invalid(format!("{plan_id} has no item {id}")))?.state = state;
            Ok(Vec::new())
        }
    }
}

/// An item's title: one line, not empty.
fn item_title(title: &str) -> Result<String, ServiceError> {
    let title = title.trim();
    if title.is_empty() || title.contains('\n') {
        return Err(invalid("an item's title is one line, not empty (what it says goes in its text)"));
    }
    Ok(title.to_owned())
}
