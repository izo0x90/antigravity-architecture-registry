//! Imports the old registry document (system_architecture.json: one JSON
//! file of components, usage trees, component types and validation tools)
//! into a plan laid over the code. Components are matched to existing code
//! where they can be; everything that can't be carried over is reported,
//! never dropped silently. Old approvals aren't carried: approvals come only
//! from users in the cockpit, so imported work starts at Declared (code that
//! already exists keeps what the code shows).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use arch_graph::{
    Component, ComponentId, Graph, ImplementationSpec, Invariant, InvariantKind, LogicStep, Plan, PlanId, ProposedLocation, Provenance, Relation,
    RelationKind, RepoPath, Schemas, SideEffect, Stage, Status, Task, ToolRef, TypeSystem, Workflow,
};
use arch_index::{PackSet, module_symbol};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ServiceError, ToolCatalog, ToolDef};

// ---- The old document, as the Python engine wrote it ----

#[derive(Deserialize)]
struct OldRegistry {
    #[serde(default)]
    components: BTreeMap<String, OldComponent>,
    #[serde(default)]
    usage_trees: BTreeMap<String, OldUsageNode>,
    #[serde(default)]
    component_types: BTreeMap<String, OldTypeRule>,
    #[serde(default)]
    validation_tools: BTreeMap<String, OldTool>,
}

#[derive(Deserialize)]
struct OldComponent {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    implements_id: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    location: Option<OldLocation>,
    #[serde(default)]
    properties: Option<Value>,
    #[serde(default)]
    inputs: Option<Value>,
    #[serde(default)]
    outputs: Option<Value>,
    #[serde(default)]
    side_effects: Vec<OldSideEffect>,
    #[serde(default)]
    implementation_spec: Option<OldSpec>,
    #[serde(default)]
    modification_tasks: Vec<OldTask>,
}

#[derive(Deserialize)]
struct OldLocation {
    file_path: String,
}

#[derive(Clone, Deserialize)]
struct OldSideEffect {
    target: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct OldSpec {
    #[serde(default)]
    pattern_or_system: Option<String>,
    #[serde(default)]
    invariants: Vec<OldInvariant>,
    #[serde(default)]
    logic_steps: Vec<OldStep>,
    #[serde(default)]
    validation: Vec<OldToolRef>,
}

#[derive(Deserialize)]
struct OldInvariant {
    name: String,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct OldStep {
    sequence: u32,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    algorithm: Option<String>,
    #[serde(default)]
    complexity: Option<String>,
}

#[derive(Deserialize)]
struct OldToolRef {
    tool_id: String,
    #[serde(default)]
    targets: Vec<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct OldTask {
    task: String,
    #[serde(default)]
    completed: bool,
}

#[derive(Deserialize)]
struct OldUsageNode {
    node_id: String,
    caller_id: String,
    component_id: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    expected_inputs: Option<Value>,
    #[serde(default)]
    expected_outputs: Option<Value>,
    #[serde(default)]
    expected_side_effects: Vec<OldSideEffect>,
    #[serde(default)]
    dependencies: Vec<OldUsageNode>,
}

#[derive(Deserialize)]
struct OldTypeRule {
    #[serde(default = "yes")]
    allows_properties: bool,
    #[serde(default = "yes")]
    allows_signature: bool,
    #[serde(default = "yes")]
    is_executable_caller: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct OldTool {
    executable: String,
    #[serde(default)]
    default_args: Vec<String>,
}

// ---- What the import produced ----

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedComponent {
    /// The id in the old document.
    pub old_id: String,
    pub id: ComponentId,
    /// Found in the code (planned as a change to it) rather than new.
    pub matched: bool,
}

/// Something from the old document that wasn't carried over as is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unmapped {
    /// What it was (e.g. `component math_validator`, `usage node n1 expected_inputs`).
    pub item: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub plan: PlanId,
    pub components: Vec<ImportedComponent>,
    pub unmapped: Vec<Unmapped>,
    /// The old validation tools, ready for `.nah/tools.toml` (not written).
    pub tools: ToolCatalog,
}

pub struct ImportInput<'a> {
    pub plan: PlanId,
    pub title: String,
    /// The old document's text.
    pub json: &'a str,
    /// Language for components with no location and nothing in the code to
    /// go by. Without it such components are reported, not imported.
    pub language: Option<String>,
}

struct Placed {
    id: ComponentId,
    /// The component's own symbol and language, and the file it is (or would be) in.
    symbol: String,
    language: String,
    file: Option<RepoPath>,
    name: String,
    matched: bool,
}

fn normalize(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The old id without its parent's prefix (`math_validator_validate` under
/// `math_validator` is `validate`).
fn leaf(old_id: &str, parent: Option<&str>) -> String {
    parent.and_then(|p| old_id.strip_prefix(p)).and_then(|rest| rest.strip_prefix('_')).filter(|rest| !rest.is_empty()).unwrap_or(old_id).to_owned()
}

/// Parents before children; parent loops are broken (and reported).
fn parent_order(components: &BTreeMap<String, OldComponent>, unmapped: &mut Vec<Unmapped>) -> (Vec<String>, BTreeSet<String>) {
    let mut order = Vec::new();
    let mut placed: BTreeSet<String> = BTreeSet::new();
    let mut broken: BTreeSet<String> = BTreeSet::new();
    for start in components.keys() {
        let mut chain: Vec<String> = Vec::new();
        let mut current = Some(start.clone());
        while let Some(id) = current {
            if placed.contains(&id) {
                break;
            }
            if chain.contains(&id) {
                unmapped.push(Unmapped { item: format!("component {id}"), reason: format!("parent loop {} -> {id}; imported without a parent", chain.join(" -> ")) });
                broken.insert(id.clone());
                break;
            }
            chain.push(id.clone());
            current = components.get(&id).and_then(|c| c.parent_id.clone()).filter(|p| components.contains_key(p));
        }
        for id in chain.into_iter().rev() {
            if placed.insert(id.clone()) {
                order.push(id);
            }
        }
    }
    (order, broken)
}

/// The type in `types` whose rules carry the same kind of thing as an old
/// type: executable or declared behavior (by signature and whether it can
/// call), or data (properties without a signature). Among those, the one
/// agreeing on most rules, then the most general, then by name. Old types
/// with neither (no signature, no properties) carry nothing to map by.
fn closest_type(types: &TypeSystem, rule: &OldTypeRule) -> Option<String> {
    let mut candidates: Vec<(u8, usize, String)> = types
        .types
        .keys()
        .filter_map(|name| types.resolve(name).ok())
        .filter(|t| {
            if rule.allows_signature {
                t.allows_signature && t.executable == rule.is_executable_caller
            } else {
                rule.allows_properties && t.allows_properties && !t.allows_signature
            }
        })
        .map(|t| {
            let agree = [t.allows_signature == rule.allows_signature, t.executable == rule.is_executable_caller, t.allows_properties == rule.allows_properties];
            (agree.iter().filter(|a| **a).count() as u8, t.lineage.len(), t.name)
        })
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
    candidates.into_iter().next().map(|(_, _, name)| name)
}

pub fn import(input: ImportInput<'_>, observed: &Graph, root: &Path, packs: &PackSet, types: &TypeSystem) -> Result<(Plan, ImportReport), ServiceError> {
    let old: OldRegistry = serde_json::from_str(input.json).map_err(|e| ServiceError::InvalidOp(format!("not an old registry document: {e}")))?;
    let mut plan = Plan::new(input.plan.0.as_str(), &input.title);
    plan.description = "Imported from the old registry document.".into();
    let mut report = ImportReport { plan: input.plan.clone(), components: Vec::new(), unmapped: Vec::new(), tools: ToolCatalog::default() };

    // Types: generic types keep their names; others map onto the closest one.
    let mut type_map: BTreeMap<String, String> = BTreeMap::new();
    let used: BTreeSet<&str> = old.components.values().map(|c| c.type_name.as_str()).collect();
    for (name, rule) in &old.component_types {
        if types.contains(name) {
            type_map.insert(name.clone(), name.clone());
            continue;
        }
        let generic = closest_type(types, rule);
        match (generic, used.contains(name.as_str())) {
            (Some(g), true) => {
                report.unmapped.push(Unmapped { item: format!("type {name}"), reason: format!("not a known type; its components are imported as '{g}' (add a specialization to keep the name)") });
                type_map.insert(name.clone(), g);
            }
            (Some(_), false) => report.unmapped.push(Unmapped { item: format!("type {name}"), reason: "not a known type and unused; its rules aren't imported".into() }),
            (None, used_here) => report.unmapped.push(Unmapped {
                item: format!("type {name}"),
                reason: format!("no type here has its rules (signature/executable or properties){}", if used_here { "; its components aren't imported" } else { "" }),
            }),
        }
    }
    for name in used {
        if !type_map.contains_key(name) && types.contains(name) {
            type_map.insert(name.to_owned(), name.to_owned());
        }
    }

    // Place every component: matched to code, or new with a derived id.
    let (order, broken) = parent_order(&old.components, &mut report.unmapped);
    let mut placed: BTreeMap<String, Placed> = BTreeMap::new();
    let mut taken: BTreeSet<ComponentId> = BTreeSet::new();
    for old_id in &order {
        let c = &old.components[old_id];
        let Some(type_name) = type_map.get(&c.type_name).cloned() else {
            if !old.component_types.contains_key(&c.type_name) {
                report.unmapped.push(Unmapped { item: format!("component {old_id}"), reason: format!("unknown type '{}'", c.type_name) });
            }
            continue;
        };
        let parent_old = c.parent_id.as_deref().filter(|_| !broken.contains(old_id));
        if let Some(p) = parent_old
            && !placed.contains_key(p)
        {
            let reason = if old.components.contains_key(p) { format!("its parent {p} wasn't imported") } else { format!("its parent {p} doesn't exist") };
            report.unmapped.push(Unmapped { item: format!("component {old_id}"), reason });
            continue;
        }
        let parent = parent_old.and_then(|p| placed.get(p));
        let file_hint = match &c.location {
            Some(loc) => match RepoPath::new(&loc.file_path) {
                Ok(path) => Some(path),
                Err(e) => {
                    report.unmapped.push(Unmapped { item: format!("component {old_id} location"), reason: e.to_string() });
                    None
                }
            },
            None => None,
        };
        let leaf_name = leaf(old_id, parent_old);
        let wanted = [normalize(&leaf_name), normalize(&c.name)];
        let fits = |code: &Component| {
            let (Ok(a), Ok(b)) = (types.resolve(&code.type_name), types.resolve(&type_name)) else { return false };
            (a.is_a(&type_name) || b.is_a(&code.type_name)) && wanted.contains(&normalize(&code.name))
        };
        let candidates: Vec<&Component> = match parent {
            Some(p) if p.matched => observed.outgoing(&p.id, Some(RelationKind::Contains)).filter_map(|r| observed.component(&r.target)).filter(|code| fits(code)).collect(),
            Some(_) => Vec::new(),
            None => observed
                .components()
                .filter(|code| code.anchor.is_some() && fits(code))
                .filter(|code| file_hint.as_ref().is_none_or(|f| code.anchor.as_ref().is_some_and(|a| &a.file == f)))
                .collect(),
        };
        let candidates: Vec<&Component> = candidates.into_iter().filter(|code| !taken.contains(&code.id)).collect();
        if candidates.len() > 1 {
            let ids: Vec<String> = candidates.iter().map(|c| c.id.to_string()).collect();
            report.unmapped.push(Unmapped { item: format!("component {old_id}"), reason: format!("matches several components in the code ({}); imported as new", ids.join(", ")) });
        }
        let place = match candidates.as_slice() {
            [code] => {
                let anchor = code.anchor.as_ref().ok_or_else(|| ServiceError::Corrupt(format!("{} matched without an anchor", code.id)))?;
                Placed { id: code.id.clone(), symbol: anchor.symbol.clone(), language: anchor.language.clone(), file: Some(anchor.file.clone()), name: code.name.clone(), matched: true }
            }
            _ => {
                let (language, file, base) = match (parent, &file_hint) {
                    (Some(p), _) => (Some(p.language.clone()), p.file.clone(), Some(p.symbol.clone())),
                    (None, Some(f)) => match packs.for_path(Path::new(f.as_str())) {
                        Some(pack) => (Some(pack.name().to_owned()), Some(f.clone()), Some(module_symbol(root, f, pack))),
                        None => {
                            report.unmapped.push(Unmapped { item: format!("component {old_id} location"), reason: format!("no language pack handles {f}") });
                            (input.language.clone(), None, None)
                        }
                    },
                    (None, None) => (input.language.clone(), None, None),
                };
                let Some(language) = language else {
                    report.unmapped.push(Unmapped {
                        item: format!("component {old_id}"),
                        reason: "no location, no parent and nothing in the code to match; give a language to import it".into(),
                    });
                    continue;
                };
                let sep = packs.by_name(&language).map(|p| p.spec.language.separator.clone());
                let symbol = match (base, sep) {
                    (Some(base), Some(sep)) => format!("{base}{sep}{leaf_name}"),
                    _ => leaf_name.clone(),
                };
                Placed { id: ComponentId::for_symbol(&language, &symbol), symbol, language, file, name: leaf_name.clone(), matched: false }
            }
        };
        if !taken.insert(place.id.clone()) {
            report.unmapped.push(Unmapped { item: format!("component {old_id}"), reason: format!("maps to {}, which another component already took", place.id) });
            continue;
        }
        placed.insert(old_id.clone(), place);
    }

    // Old ids -> names, for `title` references in schemas.
    let names: BTreeMap<String, String> = placed.iter().map(|(old_id, p)| (old_id.clone(), p.name.clone())).collect();

    for old_id in &order {
        let Some(place) = placed.get(old_id) else { continue };
        let c = &old.components[old_id];
        let type_name = type_map[&c.type_name].clone();
        let mut schema = |value: &Option<Value>, what: &str| value.as_ref().map(|v| convert_refs(v, &names, &format!("component {old_id} {what}"), &mut report.unmapped));
        let schemas = Schemas {
            properties: schema(&c.properties, "properties"),
            inputs: schema(&c.inputs, "inputs"),
            outputs: schema(&c.outputs, "outputs"),
            side_effects: c.side_effects.iter().map(|s| SideEffect { target: s.target.clone(), description: s.description.clone() }).collect(),
        };
        let spec = spec(old_id, c, &mut report.unmapped);
        let old_stage = c.stage.as_deref().unwrap_or("declared");
        let old_status = c.status.as_deref().unwrap_or("new");
        let pending = c.modification_tasks.iter().any(|t| !t.completed);
        let (status, stage) = if place.matched {
            let code_stage = observed.component(&place.id).map_or(Stage::Implemented, |code| code.stage);
            match old_status {
                "modifying" => (Status::Modifying, Stage::Declared),
                _ if pending => (Status::Modifying, Stage::Declared),
                "deprecated" => (Status::Deprecated, code_stage),
                _ => (Status::Existing, code_stage),
            }
        } else {
            if old_status != "new" {
                report.unmapped.push(Unmapped { item: format!("component {old_id} status"), reason: format!("was '{old_status}' but no matching code was found; imported as new") });
            }
            (Status::New, Stage::Declared)
        };
        if old_stage != "declared" && stage == Stage::Declared {
            report.unmapped.push(Unmapped { item: format!("component {old_id} stage"), reason: format!("was '{old_stage}'; approvals aren't imported, so it starts at declared (approve it in the cockpit)") });
        }
        let description = if normalize(&c.name) == normalize(&place.name) || c.name.trim().is_empty() {
            c.description.clone()
        } else if c.description.trim().is_empty() {
            c.name.clone()
        } else {
            format!("{}: {}", c.name, c.description)
        };
        let parent = c.parent_id.as_ref().and_then(|p| placed.get(p)).map(|p| p.id.clone());
        let proposed = match (&place.file, place.matched) {
            (Some(file), false) => Some(ProposedLocation { language: place.language.clone(), file: file.clone(), parent: parent.clone(), symbol: place.symbol.clone() }),
            _ => None,
        };
        plan.components.push(Component {
            id: place.id.clone(),
            name: place.name.clone(),
            type_name,
            description,
            status,
            stage,
            anchor: None,
            proposed,
            signature: None,
            schemas,
            spec,
            placeholders: Vec::new(),
        });
        report.components.push(ImportedComponent { old_id: old_id.clone(), id: place.id.clone(), matched: place.matched });
        if let Some(parent) = parent {
            plan.relations.push(Relation::new(RelationKind::Contains, parent, place.id.clone(), Provenance::PLANNED));
        }
        if let Some(target) = &c.implements_id {
            match placed.get(target) {
                Some(t) => plan.relations.push(Relation::new(RelationKind::Implements, place.id.clone(), t.id.clone(), Provenance::PLANNED)),
                None => report.unmapped.push(Unmapped { item: format!("component {old_id} implements"), reason: format!("{target} wasn't imported") }),
            }
        }
    }

    // Usage trees: ordered calls, and each root becomes a workflow.
    let lookup = Lookup { placed: &placed, names: &names, components: &old.components, type_map: &type_map, types };
    for (tree, root_node) in &old.usage_trees {
        let entry = placed.get(&root_node.caller_id).map(|p| p.id.clone());
        match entry {
            Some(entry) => plan.workflows.push(Workflow { name: tree.clone(), entry, description: root_node.description.clone().unwrap_or_default() }),
            None => report.unmapped.push(Unmapped { item: format!("usage tree {tree}"), reason: format!("its caller {} wasn't imported", root_node.caller_id) }),
        }
        calls(root_node, 0, &lookup, &mut plan, &mut report.unmapped);
    }

    for (name, tool) in &old.validation_tools {
        report.tools.tools.insert(
            name.clone(),
            ToolDef { description: String::new(), executable: tool.executable.clone(), default_args: tool.default_args.clone(), applies_to: Vec::new(), timeout_secs: 300 },
        );
    }
    let mut seen = BTreeSet::new();
    plan.relations.retain(|r| seen.insert(r.id()));
    Ok((plan, report))
}

/// What usage nodes are resolved against.
struct Lookup<'a> {
    placed: &'a BTreeMap<String, Placed>,
    /// Old ids -> imported names (for `title` references in expectations).
    names: &'a BTreeMap<String, String>,
    components: &'a BTreeMap<String, OldComponent>,
    type_map: &'a BTreeMap<String, String>,
    types: &'a TypeSystem,
}

fn calls(node: &OldUsageNode, order: u32, cx: &Lookup<'_>, plan: &mut Plan, unmapped: &mut Vec<Unmapped>) {
    let (placed, components, type_map, types) = (cx.placed, cx.components, cx.type_map, cx.types);
    let item = format!("usage node {}", node.node_id);
    match (placed.get(&node.caller_id), placed.get(&node.component_id)) {
        (Some(caller), Some(callee)) => {
            let caller_type = components.get(&node.caller_id).and_then(|c| type_map.get(&c.type_name));
            let executable = caller_type.and_then(|t| types.resolve(t).ok()).is_some_and(|t| t.executable);
            if executable {
                let mut call = Relation::new(RelationKind::Calls, caller.id.clone(), callee.id.clone(), Provenance::PLANNED);
                call.order = Some(order);
                call.note = node.description.clone().unwrap_or_default();
                // What the call site expects travels with the call.
                let mut schema = |value: &Option<Value>, what: &str| value.as_ref().map(|v| convert_refs(v, cx.names, &format!("{item} {what}"), unmapped));
                let expects = Schemas {
                    properties: None,
                    inputs: schema(&node.expected_inputs, "expected_inputs"),
                    outputs: schema(&node.expected_outputs, "expected_outputs"),
                    side_effects: node.expected_side_effects.iter().map(|s| SideEffect { target: s.target.clone(), description: s.description.clone() }).collect(),
                };
                call.expects = (!expects.is_empty()).then_some(expects);
                plan.relations.push(call);
            } else {
                unmapped.push(Unmapped { item: item.clone(), reason: format!("{} can't call anything (not executable)", caller.id) });
            }
        }
        (caller, callee) => {
            let missing: Vec<&str> = [(caller.is_none(), node.caller_id.as_str()), (callee.is_none(), node.component_id.as_str())].into_iter().filter(|(gone, _)| *gone).map(|(_, id)| id).collect();
            unmapped.push(Unmapped { item: item.clone(), reason: format!("{} wasn't imported", missing.join(" and ")) });
        }
    }
    for (i, child) in node.dependencies.iter().enumerate() {
        calls(child, i as u32, cx, plan, unmapped);
    }
}

fn spec(old_id: &str, c: &OldComponent, unmapped: &mut Vec<Unmapped>) -> Option<ImplementationSpec> {
    let tasks: Vec<Task> = c.modification_tasks.iter().map(|t| Task { text: t.task.clone(), done: t.completed }).collect();
    let Some(old) = &c.implementation_spec else {
        return (!tasks.is_empty()).then(|| ImplementationSpec { tasks, ..ImplementationSpec::default() });
    };
    let invariants = old
        .invariants
        .iter()
        .map(|i| {
            let kind = match i.kind.as_deref() {
                None => InvariantKind::default(),
                Some(k) => match serde_json::from_value(Value::String(k.to_owned())) {
                    Ok(kind) => kind,
                    Err(_) => {
                        unmapped.push(Unmapped { item: format!("component {old_id} invariant {}", i.name), reason: format!("unknown kind '{k}'; imported as {}", InvariantKind::default()) });
                        InvariantKind::default()
                    }
                },
            };
            Invariant { name: i.name.clone(), kind, description: i.description.clone() }
        })
        .collect();
    Some(ImplementationSpec {
        pattern: old.pattern_or_system.clone(),
        logic_steps: old
            .logic_steps
            .iter()
            .map(|s| LogicStep { sequence: s.sequence, name: s.name.clone(), description: s.description.clone(), algorithm: s.algorithm.clone(), complexity: s.complexity.clone(), check: None })
            .collect(),
        invariants,
        tasks,
        validation: old.validation.iter().map(|v| ToolRef { tool: v.tool_id.clone(), targets: v.targets.clone(), args: v.args.clone() }).collect(),
    })
}

/// Old schemas name other components by `title` (an old id); here a `$ref`
/// to the imported component's name.
fn convert_refs(schema: &Value, names: &BTreeMap<String, String>, at: &str, unmapped: &mut Vec<Unmapped>) -> Value {
    match schema {
        Value::Object(map) => {
            if let Some(Value::String(title)) = map.get("title")
                && map.get("properties").is_none()
            {
                match names.get(title) {
                    Some(name) => return serde_json::json!({ "$ref": name }),
                    None => unmapped.push(Unmapped { item: at.to_owned(), reason: format!("refers to '{title}', which wasn't imported; kept as written") }),
                }
            }
            Value::Object(map.iter().map(|(k, v)| (k.clone(), convert_refs(v, names, at, unmapped))).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| convert_refs(v, names, at, unmapped)).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // req: P3.6
    #[test]
    fn old_types_map_onto_whatever_types_the_project_defines() {
        // A type system with none of the default names: the importer must pick
        // by rules, not by name.
        let types = TypeSystem::from_toml(
            r#"
    [types.pkg]
    allowed_parents = ["pkg"]
    executable = true
    allows_signature = false
    allows_properties = false
    allows_implementation_spec = true
    [types.routine]
    allowed_parents = ["pkg", "record"]
    executable = true
    allows_signature = true
    allows_properties = false
    allows_implementation_spec = true
    [types.contract_op]
    allowed_parents = ["pkg"]
    executable = false
    allows_signature = true
    allows_properties = false
    allows_implementation_spec = false
    [types.record]
    allowed_parents = ["pkg"]
    executable = false
    allows_signature = false
    allows_properties = true
    allows_implementation_spec = false
    [types.fancy_record]
    extends = "record"
    "#,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let old = serde_json::json!({
            "components": {
                "act": {"id": "act", "name": "act", "type": "function", "description": "Does it."},
                "spec_op": {"id": "spec_op", "name": "spec_op", "type": "operation", "description": "Declared."},
                "dto": {"id": "dto", "name": "Dto", "type": "data_object", "description": "Data."}
            },
            "component_types": {
                "function": {"allows_properties": false, "allows_signature": true, "is_executable_caller": true},
                "operation": {"allows_properties": false, "allows_signature": true, "is_executable_caller": false},
                "data_object": {"allows_properties": true, "allows_signature": false, "is_executable_caller": false}
            }
        })
        .to_string();
        let input = ImportInput { plan: PlanId::new("mapped").unwrap(), title: "Mapped".into(), json: &old, language: Some("rust".into()) };
        let (plan, report) = import(input, &Graph::new(), dir.path(), &arch_index::PackSet::from_shipped_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")).unwrap(), &types).unwrap();
        let type_of = |old: &str| plan.component(&report.components.iter().find(|c| c.old_id == old).unwrap().id).unwrap().type_name.clone();
        assert_eq!((type_of("act"), type_of("spec_op"), type_of("dto")), ("routine".to_owned(), "contract_op".to_owned(), "record".to_owned()), "the most general type with the same rules");
        let reasons: Vec<String> = report.unmapped.iter().map(|u| format!("{}: {}", u.item, u.reason)).collect();
        assert!(reasons.iter().any(|r| r.starts_with("type function:") && r.contains("'routine'")), "each mapping is reported: {reasons:#?}");
    }
}
