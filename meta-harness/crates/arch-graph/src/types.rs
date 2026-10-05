//! The component type system: generic default types from config, replaceable,
//! with specialized types that extend a base and inherit its rules.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ModelError;

const DEFAULT_TYPES: &str = include_str!("../config/types.toml");

/// A detail a type can require before approval. Unknown names are rejected
/// when the type system loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredField {
    Description,
    /// A language signature or input/output schemas.
    Signature,
    Inputs,
    Outputs,
    Properties,
    SideEffects,
    Spec,
    Pattern,
    LogicSteps,
    Invariants,
    Validation,
}

impl RequiredField {
    pub const ALL: [RequiredField; 11] = [
        Self::Description,
        Self::Signature,
        Self::Inputs,
        Self::Outputs,
        Self::Properties,
        Self::SideEffects,
        Self::Spec,
        Self::Pattern,
        Self::LogicSteps,
        Self::Invariants,
        Self::Validation,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Description => "description",
            Self::Signature => "signature",
            Self::Inputs => "inputs",
            Self::Outputs => "outputs",
            Self::Properties => "properties",
            Self::SideEffects => "side_effects",
            Self::Spec => "spec",
            Self::Pattern => "pattern",
            Self::LogicSteps => "logic_steps",
            Self::Invariants => "invariants",
            Self::Validation => "validation",
        }
    }
}

impl std::fmt::Display for RequiredField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// A type as written in config; unset rules inherit from `extends`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TypeDef {
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_parents: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allows_signature: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allows_properties: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allows_implementation_spec: Option<bool>,
    /// Added to (not replacing) the base type's required fields.
    #[serde(default)]
    pub required_fields: Vec<RequiredField>,
}

/// A type with its inheritance resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedType {
    pub name: String,
    /// This type, then its bases, nearest first.
    pub lineage: Vec<String>,
    pub description: String,
    pub allowed_parents: Vec<String>,
    pub executable: bool,
    pub allows_signature: bool,
    pub allows_properties: bool,
    pub allows_implementation_spec: bool,
    pub required_fields: Vec<RequiredField>,
}

impl ResolvedType {
    /// True when this type is `name` or specializes it.
    pub fn is_a(&self, name: &str) -> bool {
        self.lineage.iter().any(|t| t == name)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeSystem {
    pub types: BTreeMap<String, TypeDef>,
}

impl TypeSystem {
    /// The built-in generic types (config/types.toml, checked by tests).
    pub fn defaults() -> Result<Self, ModelError> {
        Self::from_toml(DEFAULT_TYPES)
    }

    pub fn from_toml(text: &str) -> Result<Self, ModelError> {
        let system: TypeSystem = toml::from_str(text).map_err(|e| ModelError::Config(e.to_string()))?;
        system.check()?;
        Ok(system)
    }

    /// Adds or replaces types from TOML that may extend the types already
    /// here (a project's `.nah/types.toml`); checked as a whole.
    pub fn extend_from_toml(&mut self, text: &str) -> Result<(), ModelError> {
        let other: TypeSystem = toml::from_str(text).map_err(|e| ModelError::Config(e.to_string()))?;
        self.extend_with(other)
    }

    /// Adds or replaces types (a project's own types or specializations).
    pub fn extend_with(&mut self, other: TypeSystem) -> Result<(), ModelError> {
        self.types.extend(other.types);
        self.check()
    }

    /// Every `extends` and `allowed_parents` names a known type, with no cycles.
    fn check(&self) -> Result<(), ModelError> {
        for (name, def) in &self.types {
            self.resolve(name)?;
            for parent in def.allowed_parents.iter().flatten() {
                if !self.types.contains_key(parent) {
                    return Err(ModelError::UnknownType { name: parent.clone(), context: format!("allowed parent of '{name}'") });
                }
            }
        }
        Ok(())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.types.contains_key(name)
    }

    pub fn resolve(&self, name: &str) -> Result<ResolvedType, ModelError> {
        let mut lineage: Vec<String> = Vec::new();
        let mut current = Some(name.to_owned());
        while let Some(type_name) = current {
            if lineage.contains(&type_name) {
                lineage.push(type_name);
                return Err(ModelError::TypeCycle(lineage.join(" -> ")));
            }
            let def = self
                .types
                .get(&type_name)
                .ok_or_else(|| ModelError::UnknownType { name: type_name.clone(), context: format!("resolving '{name}'") })?;
            lineage.push(type_name);
            current = def.extends.clone();
        }
        // Nearest definition wins for each rule; required fields accumulate.
        let defs: Vec<&TypeDef> = lineage.iter().filter_map(|t| self.types.get(t)).collect();
        let pick = |f: fn(&TypeDef) -> Option<bool>| defs.iter().find_map(|d| f(d)).unwrap_or(false);
        let mut required: Vec<RequiredField> = Vec::new();
        for def in defs.iter().rev() {
            for field in &def.required_fields {
                if !required.contains(field) {
                    required.push(*field);
                }
            }
        }
        Ok(ResolvedType {
            name: name.to_owned(),
            description: defs.iter().map(|d| d.description.as_str()).find(|d| !d.is_empty()).unwrap_or_default().to_owned(),
            allowed_parents: defs.iter().find_map(|d| d.allowed_parents.clone()).unwrap_or_default(),
            executable: pick(|d| d.executable),
            allows_signature: pick(|d| d.allows_signature),
            allows_properties: pick(|d| d.allows_properties),
            allows_implementation_spec: pick(|d| d.allows_implementation_spec),
            required_fields: required,
            lineage,
        })
    }

    /// Whether a component of type `child` may be contained in one of type
    /// `parent` (a specialized parent counts as its bases).
    pub fn may_contain(&self, parent: &str, child: &str) -> Result<bool, ModelError> {
        let child = self.resolve(child)?;
        let parent = self.resolve(parent)?;
        Ok(child.allowed_parents.iter().any(|allowed| parent.is_a(allowed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // req: P2.1
    #[test]
    fn defaults_cover_the_generic_concepts() {
        let types = TypeSystem::defaults().unwrap();
        for name in ["module", "class", "interface", "function", "operation"] {
            assert!(types.contains(name), "{name}");
        }
        let function = types.resolve("function").unwrap();
        assert!(function.executable && function.allows_signature);
        assert!(types.may_contain("class", "function").unwrap());
        assert!(!types.may_contain("function", "module").unwrap());
    }

    // req: P2.1
    #[test]
    fn specialized_types_inherit_and_override() {
        let mut types = TypeSystem::defaults().unwrap();
        let extra = TypeSystem::from_toml(
            r#"
            [types.endpoint]
            extends = "function"
            description = "An HTTP handler."
            required_fields = ["logic_steps"]
            [types.repository]
            extends = "class"
            allows_implementation_spec = true
            "#,
        );
        // `endpoint` alone references a base that isn't in this file.
        assert!(matches!(extra, Err(ModelError::UnknownType { .. })));
        let extra: TypeSystem = toml::from_str(
            r#"
            [types.endpoint]
            extends = "function"
            description = "An HTTP handler."
            required_fields = ["logic_steps"]
            [types.repository]
            extends = "class"
            allows_implementation_spec = true
            "#,
        )
        .unwrap();
        types.extend_with(extra).unwrap();
        let endpoint = types.resolve("endpoint").unwrap();
        assert_eq!(endpoint.lineage, ["endpoint", "function"]);
        assert!(endpoint.executable && endpoint.allows_signature);
        assert_eq!(endpoint.required_fields, [RequiredField::Description, RequiredField::Signature, RequiredField::LogicSteps]);
        // Only details NAH knows how to check can be required.
        let unknown = TypeSystem::from_toml("[types.x]\nrequired_fields = [\"route\"]");
        assert!(matches!(&unknown, Err(ModelError::Config(m)) if m.contains("route")), "{unknown:?}");
        assert!(endpoint.is_a("function"));
        let repository = types.resolve("repository").unwrap();
        assert!(repository.allows_implementation_spec && repository.allows_properties);
        // A specialized parent counts as its base: functions may live in a repository.
        assert!(types.may_contain("repository", "function").unwrap());
    }

    // req: P2.1
    #[test]
    fn unknown_types_and_inheritance_cycles_are_errors() {
        let types = TypeSystem::defaults().unwrap();
        assert!(matches!(types.resolve("widget"), Err(ModelError::UnknownType { .. })));
        let cyclic = TypeSystem::from_toml("[types.a]\nextends = \"b\"\n[types.b]\nextends = \"a\"\n");
        assert!(matches!(cyclic, Err(ModelError::TypeCycle(_))));
        let bad_parent = TypeSystem::from_toml("[types.a]\nallowed_parents = [\"nope\"]\n");
        assert!(matches!(bad_parent, Err(ModelError::UnknownType { .. })));
    }
}
