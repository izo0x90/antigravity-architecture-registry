//! Schemas: JSON Schema for data shapes (inputs, outputs, properties), a
//! shorthand DSL that compiles to it, `$ref`s that resolve by title to named
//! data types, and structural compatibility between two schemas.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::ModelError;

const DEFAULT_DSL: &str = include_str!("../config/schema_dsl.toml");

/// JSON Schema type names (what DSL aliases map onto; `any` means open).
const JSON_TYPES: [&str; 8] = ["string", "integer", "number", "boolean", "object", "array", "null", "any"];

/// JSON Schema keywords. A map is taken as JSON Schema (not a shorthand
/// field map) only when every key is one of these and it says what it is
/// (a type, a reference, or a combinator), so a record with a field called
/// `type` stays a record.
const SCHEMA_KEYWORDS: [&str; 32] = [
    "$schema", "$id", "$ref", "$defs", "$comment", "definitions", "type", "title", "description", "default", "examples", "enum", "const",
    "anyOf", "oneOf", "allOf", "not", "properties", "required", "additionalProperties", "patternProperties", "items", "prefixItems",
    "minItems", "maxItems", "uniqueItems", "minimum", "maximum", "minLength", "maxLength", "pattern", "format",
];

/// Keywords that say what a schema is.
const SCHEMA_SHAPES: [&str; 6] = ["$ref", "anyOf", "oneOf", "allOf", "enum", "const"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dsl {
    /// Alias -> JSON Schema type.
    pub primitives: BTreeMap<String, String>,
}

impl Dsl {
    pub fn defaults() -> Result<Self, ModelError> {
        Self::from_toml(DEFAULT_DSL)
    }

    pub fn from_toml(text: &str) -> Result<Self, ModelError> {
        let dsl: Dsl = toml::from_str(text).map_err(|e| ModelError::Config(e.to_string()))?;
        dsl.check()?;
        Ok(dsl)
    }

    /// Adds or replaces aliases.
    pub fn extend_with(&mut self, other: Dsl) -> Result<(), ModelError> {
        self.primitives.extend(other.primitives);
        self.check()
    }

    fn check(&self) -> Result<(), ModelError> {
        match self.primitives.iter().find(|(_, t)| !JSON_TYPES.contains(&t.as_str())) {
            Some((alias, t)) => Err(ModelError::Config(format!("schema alias '{alias}' maps to '{t}', not a JSON Schema type"))),
            None => Ok(()),
        }
    }

    /// Compiles shorthand (a type string or a field map) to JSON Schema.
    /// Anything that is already JSON Schema passes through (its nested
    /// `properties`/`items` may still use shorthand).
    pub fn compile(&self, shorthand: &Value) -> Result<Value, ModelError> {
        match shorthand {
            Value::String(text) => self.compile_type(text),
            Value::Object(map) if is_schema(map) => {
                let mut out = map.clone();
                if let Some(Value::Object(props)) = map.get("properties") {
                    let mut compiled = Map::new();
                    for (name, field) in props {
                        compiled.insert(name.clone(), self.compile(field)?);
                    }
                    out.insert("properties".into(), Value::Object(compiled));
                }
                if let Some(items) = map.get("items") {
                    out.insert("items".into(), self.compile(items)?);
                }
                for key in ["anyOf", "oneOf", "allOf"] {
                    if let Some(Value::Array(options)) = map.get(key) {
                        out.insert(key.into(), Value::Array(options.iter().map(|o| self.compile(o)).collect::<Result<_, _>>()?));
                    }
                }
                Ok(Value::Object(out))
            }
            Value::Object(fields) => {
                let mut properties = Map::new();
                let mut required = Vec::new();
                for (name, field) in fields {
                    let optional = field.as_str().is_some_and(|t| t.trim().ends_with('?'));
                    properties.insert(name.clone(), self.compile(field)?);
                    if !optional {
                        required.push(Value::String(name.clone()));
                    }
                }
                let mut schema = json!({"type": "object", "properties": properties});
                if !required.is_empty() {
                    schema["required"] = Value::Array(required);
                }
                Ok(schema)
            }
            other => Err(ModelError::Format(format!("schema shorthand must be a type string or a field map, got {other}"))),
        }
    }

    fn compile_type(&self, raw: &str) -> Result<Value, ModelError> {
        let mut text = raw.trim();
        let optional = text.ends_with('?');
        if optional {
            text = text[..text.len() - 1].trim_end();
        }
        let mut arrays = 0;
        while let Some(inner) = text.strip_suffix("[]") {
            arrays += 1;
            text = inner.trim_end();
        }
        let mut schema = match self.primitives.get(text) {
            Some(t) if t == "any" => json!({}),
            Some(t) => json!({"type": t}),
            None if is_identifier(text) => json!({"$ref": text}),
            None => return Err(ModelError::Format(format!("'{raw}': '{text}' is neither a known type nor a type name"))),
        };
        for _ in 0..arrays {
            schema = json!({"type": "array", "items": schema});
        }
        if optional {
            schema = json!({"anyOf": [schema, {"type": "null"}]});
        }
        Ok(schema)
    }
}

/// Whether `value` has the JSON type JSON Schema gives `keyword`.
fn keyword_fits(keyword: &str, value: &Value) -> bool {
    match keyword {
        "type" => value.is_string() || value.is_array(),
        "enum" | "required" | "examples" | "anyOf" | "oneOf" | "allOf" | "prefixItems" => value.is_array(),
        "properties" | "$defs" | "definitions" | "patternProperties" | "not" => value.is_object(),
        "items" | "additionalProperties" => value.is_object() || value.is_boolean(),
        "minItems" | "maxItems" | "minimum" | "maximum" | "minLength" | "maxLength" => value.is_number(),
        "uniqueItems" => value.is_boolean(),
        "default" | "const" => true,
        _ => value.is_string(),
    }
}

/// A map is JSON Schema (not a shorthand record) when it says what it is and
/// every key is a schema keyword holding the right kind of value. A record
/// whose field names are all schema keywords must be written as full schema
/// (`{type = "object", properties = {...}}`).
fn is_schema(map: &Map<String, Value>) -> bool {
    let typed = map.get("type").and_then(Value::as_str).is_some_and(|t| JSON_TYPES.contains(&t));
    let shaped = typed || SCHEMA_SHAPES.iter().any(|k| map.contains_key(*k));
    shaped && map.iter().all(|(k, v)| SCHEMA_KEYWORDS.contains(&k.as_str()) && keyword_fits(k, v))
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_') && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == ':')
}

/// The title a `$ref` points at (`Name`, `#/Name`, `#/$defs/Name`, `#/definitions/Name`).
pub fn ref_title(reference: &str) -> &str {
    reference.rsplit('/').next().unwrap_or(reference)
}

/// Every `$ref` title used anywhere in a schema, in order of appearance.
pub fn refs(schema: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_refs(schema, &mut out);
    out
}

fn collect_refs(schema: &Value, out: &mut Vec<String>) {
    match schema {
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get("$ref") {
                out.push(ref_title(r).to_owned());
            }
            map.values().for_each(|v| collect_refs(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| collect_refs(v, out)),
        _ => {}
    }
}

/// Named data types by title: what `$ref`s resolve to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchemaSet {
    named: BTreeMap<String, Value>,
}

/// Why a schema couldn't be resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefError {
    Missing(String),
    /// The loop, starting and ending at the same title.
    Cycle(Vec<String>),
}

impl SchemaSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, title: impl Into<String>, schema: Value) {
        self.named.insert(title.into(), schema);
    }

    pub fn contains(&self, title: &str) -> bool {
        self.named.contains_key(title)
    }

    pub fn get(&self, title: &str) -> Option<&Value> {
        self.named.get(title)
    }

    /// Titles referenced by `schema` that aren't defined.
    pub fn missing(&self, schema: &Value) -> Vec<String> {
        let mut seen = BTreeSet::new();
        refs(schema).into_iter().filter(|r| !self.named.contains_key(r) && seen.insert(r.clone())).collect()
    }

    /// Every reference loop among the named types, each once, as a path that
    /// starts and ends at the same title (smallest title first).
    pub fn cycles(&self) -> Vec<Vec<String>> {
        let edges: BTreeMap<String, Vec<String>> = self.named.iter().map(|(t, s)| (t.clone(), refs(s))).collect();
        crate::cycles::loops(&edges)
    }

    /// `schema` with every `$ref` replaced by the named type it points at.
    pub fn resolve(&self, schema: &Value) -> Result<Value, RefError> {
        self.resolve_inner(schema, &mut Vec::new())
    }

    fn resolve_inner(&self, schema: &Value, visiting: &mut Vec<String>) -> Result<Value, RefError> {
        match schema {
            Value::Object(map) => {
                if let Some(Value::String(r)) = map.get("$ref") {
                    let title = ref_title(r).to_owned();
                    if let Some(at) = visiting.iter().position(|t| *t == title) {
                        let mut cycle = visiting[at..].to_vec();
                        cycle.push(title);
                        return Err(RefError::Cycle(cycle));
                    }
                    let target = self.named.get(&title).ok_or(RefError::Missing(title.clone()))?;
                    visiting.push(title);
                    let resolved = self.resolve_inner(target, visiting);
                    visiting.pop();
                    return resolved;
                }
                let mut out = Map::new();
                for (k, v) in map {
                    out.insert(k.clone(), self.resolve_inner(v, visiting)?);
                }
                Ok(Value::Object(out))
            }
            Value::Array(items) => Ok(Value::Array(items.iter().map(|v| self.resolve_inner(v, visiting)).collect::<Result<_, _>>()?)),
            other => Ok(other.clone()),
        }
    }
}

/// Whether data shaped like `provided` satisfies `accepted`: types, required
/// fields, nested objects, array items and anyOf. Both are resolved schemas.
/// Returns one line per mismatch, with the path where it happens.
pub fn compatibility(provided: &Value, accepted: &Value) -> Vec<String> {
    let mut out = Vec::new();
    compare(provided, accepted, "", &mut out);
    out
}

fn compare(provided: &Value, accepted: &Value, path: &str, out: &mut Vec<String>) {
    let at = if path.is_empty() { "value".to_owned() } else { path.to_owned() };
    let open = |s: &Value| s.as_object().is_none_or(|m| m.is_empty());
    if open(accepted) {
        return; // accepts anything
    }
    if open(provided) {
        out.push(format!("{at}: shape unspecified where {} is expected", describe(accepted)));
        return;
    }
    // Every option the provider may produce must be acceptable.
    if let Some(options) = provided.get("anyOf").and_then(Value::as_array) {
        for (i, option) in options.iter().enumerate() {
            let mut sub = Vec::new();
            compare(option, accepted, path, &mut sub);
            if !sub.is_empty() {
                out.push(format!("{at}: option {i} ({}) is not accepted: {}", describe(option), sub.join("; ")));
            }
        }
        return;
    }
    // At least one accepted option must take what's provided.
    if let Some(options) = accepted.get("anyOf").and_then(Value::as_array) {
        let mut reasons = Vec::new();
        for option in options {
            let mut sub = Vec::new();
            compare(provided, option, path, &mut sub);
            if sub.is_empty() {
                return;
            }
            reasons.push(sub.join("; "));
        }
        out.push(format!("{at}: {} matches none of the accepted options ({})", describe(provided), reasons.join(" | ")));
        return;
    }
    let (pt, at_type) = (provided.get("type").and_then(Value::as_str), accepted.get("type").and_then(Value::as_str));
    let widens = matches!((pt, at_type), (Some("integer"), Some("number")));
    if pt != at_type && !widens {
        out.push(format!("{at}: type {} where {} is expected", pt.unwrap_or("unspecified"), at_type.unwrap_or("unspecified")));
        return;
    }
    match at_type {
        Some("object") => {
            let empty = Map::new();
            let pprops = provided.get("properties").and_then(Value::as_object).unwrap_or(&empty);
            let aprops = accepted.get("properties").and_then(Value::as_object).unwrap_or(&empty);
            let prequired: BTreeSet<&str> = provided.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
            for name in accepted.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                if !pprops.contains_key(name) {
                    out.push(format!("{}: required field missing", join(path, name)));
                } else if !prequired.contains(name) {
                    out.push(format!("{}: required but optional where it comes from", join(path, name)));
                }
            }
            for (name, accepted_field) in aprops {
                if let Some(provided_field) = pprops.get(name) {
                    compare(provided_field, accepted_field, &join(path, name), out);
                }
            }
        }
        Some("array") => {
            let items = |s: &Value| s.get("items").cloned().unwrap_or(json!({}));
            compare(&items(provided), &items(accepted), &format!("{at}[]"), out);
        }
        _ => {}
    }
}

fn join(path: &str, field: &str) -> String {
    if path.is_empty() { field.to_owned() } else { format!("{path}.{field}") }
}

fn describe(schema: &Value) -> String {
    match schema.get("type").and_then(Value::as_str) {
        Some(t) => t.to_owned(),
        None if schema.get("anyOf").is_some() => "a union".to_owned(),
        None => "an unspecified shape".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // req: P2.5
    #[test]
    fn shorthand_compiles_to_json_schema_and_refs_resolve_by_title() {
        let dsl = Dsl::defaults().unwrap();
        assert_eq!(dsl.compile(&json!("int")).unwrap(), json!({"type": "integer"}));
        assert_eq!(dsl.compile(&json!("string[]")).unwrap(), json!({"type": "array", "items": {"type": "string"}}));
        assert_eq!(dsl.compile(&json!("User?")).unwrap(), json!({"anyOf": [{"$ref": "User"}, {"type": "null"}]}));
        assert_eq!(dsl.compile(&json!("float[][]")).unwrap()["items"]["items"], json!({"type": "number"}));
        let compiled = dsl.compile(&json!({"id": "int", "tags": "str[]", "note": "text?"})).unwrap();
        assert_eq!(compiled["type"], "object");
        assert_eq!(compiled["properties"]["tags"], json!({"type": "array", "items": {"type": "string"}}));
        assert_eq!(compiled["required"], json!(["id", "tags"]), "optional fields aren't required");
        // Real JSON Schema passes through, with shorthand inside still compiled.
        let mixed = dsl.compile(&json!({"type": "object", "properties": {"n": "int"}})).unwrap();
        assert_eq!(mixed["properties"]["n"], json!({"type": "integer"}));
        // A record with a field called `type` is still a record.
        let event = dsl.compile(&json!({"type": "string", "payload": "object", "at": "int?"})).unwrap();
        assert_eq!(event["type"], "object");
        assert_eq!(event["properties"]["type"], json!({"type": "string"}));
        assert_eq!(event["properties"]["payload"], json!({"type": "object"}));
        let mut required: Vec<&str> = event["required"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        required.sort();
        assert_eq!(required, ["payload", "type"]);
        // A record whose field is named like a keyword, with a non-schema value, stays a record.
        let record = dsl.compile(&json!({"enum": "string"})).unwrap();
        assert_eq!(record["properties"]["enum"], json!({"type": "string"}));
        // Written as full schema, keyword-named fields are fine too.
        let full = dsl.compile(&json!({"type": "object", "properties": {"type": "string", "description": "string"}})).unwrap();
        assert_eq!(full["properties"]["description"], json!({"type": "string"}));
        assert!(dsl.compile(&json!("not a type!")).is_err());
        assert!(dsl.compile(&json!(3)).is_err());
        // Aliases are data: projects add their own; bad targets are rejected.
        let mut custom = Dsl::defaults().unwrap();
        custom.extend_with(Dsl::from_toml("[primitives]\nuuid = \"string\"").unwrap()).unwrap();
        assert_eq!(custom.compile(&json!("uuid")).unwrap(), json!({"type": "string"}));
        assert!(Dsl::from_toml("[primitives]\nx = \"widget\"").is_err());

        let mut set = SchemaSet::new();
        set.insert("User", dsl.compile(&json!({"id": "int", "address": "Address"})).unwrap());
        set.insert("Address", dsl.compile(&json!({"city": "string"})).unwrap());
        let resolved = set.resolve(&json!({"$ref": "#/$defs/User"})).unwrap();
        assert_eq!(resolved["properties"]["address"]["properties"]["city"], json!({"type": "string"}));
        assert_eq!(set.missing(&json!({"type": "array", "items": {"$ref": "Order"}})), vec!["Order".to_owned()]);
        assert_eq!(set.resolve(&json!({"$ref": "Order"})), Err(RefError::Missing("Order".into())));
    }

    // req: P2.5
    #[test]
    fn schema_ref_cycles_are_reported_with_their_loop() {
        let dsl = Dsl::defaults().unwrap();
        let mut set = SchemaSet::new();
        set.insert("A", dsl.compile(&json!({"b": "B"})).unwrap());
        set.insert("B", dsl.compile(&json!({"a": "A[]"})).unwrap());
        set.insert("C", dsl.compile(&json!({"c": "C?"})).unwrap());
        set.insert("D", dsl.compile(&json!({"a": "A"})).unwrap());
        assert_eq!(set.cycles(), vec![vec!["A".to_owned(), "B".into(), "A".into()], vec!["C".to_owned(), "C".into()]]);
        assert_eq!(set.resolve(&json!({"$ref": "D"})), Err(RefError::Cycle(vec!["A".into(), "B".into(), "A".into()])));
    }

    #[test]
    fn compatibility_reports_type_required_nested_array_and_anyof_mismatches() {
        let dsl = Dsl::defaults().unwrap();
        let s = |v: Value| dsl.compile(&v).unwrap();
        assert!(compatibility(&s(json!({"id": "int", "name": "str"})), &s(json!({"id": "int"}))).is_empty(), "extra fields are fine");
        assert!(compatibility(&s(json!("int")), &s(json!("float"))).is_empty(), "integers are numbers");
        assert_eq!(compatibility(&s(json!("str")), &s(json!("int"))), vec!["value: type string where integer is expected"]);
        assert_eq!(compatibility(&s(json!({"name": "str"})), &s(json!({"id": "int"}))), vec!["id: required field missing"]);
        assert_eq!(compatibility(&s(json!({"id": "int?"})), &s(json!({"id": "int"})))[0], "id: required but optional where it comes from");
        assert_eq!(compatibility(&s(json!({"user": {"id": "str"}})), &s(json!({"user": {"id": "int"}}))), vec!["user.id: type string where integer is expected"]);
        assert_eq!(compatibility(&s(json!("str[]")), &s(json!("int[]"))), vec!["value[]: type string where integer is expected"]);
        // A nullable value isn't accepted where null isn't; the reverse is fine.
        let nullable = compatibility(&s(json!("int?")), &s(json!("int")));
        assert_eq!(nullable.len(), 1);
        assert!(nullable[0].starts_with("value: option 1 (null) is not accepted"), "{nullable:?}");
        assert!(compatibility(&s(json!("int")), &s(json!("int?"))).is_empty());
        let none = compatibility(&s(json!("str")), &s(json!({"anyOf": ["int", "bool"]})));
        assert!(none[0].contains("matches none of the accepted options"), "{none:?}");
        assert!(compatibility(&json!({}), &s(json!("int")))[0].contains("shape unspecified"));
        assert!(compatibility(&s(json!("int")), &json!({})).is_empty(), "an open schema accepts anything");
    }
}
