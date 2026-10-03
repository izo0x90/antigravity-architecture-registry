//! Whether a type as written in code carries data of a JSON Schema shape,
//! using the language pack's type tables (no language specifics here).

use serde_json::Value;

use crate::pack::LanguageSpec;

/// The words of a written type, minus the pack's noise words, with path
/// prefixes dropped (`crate::x::User` -> `User`).
fn words(written: &str, lang: &LanguageSpec) -> Vec<String> {
    const PATH: char = '\u{1}';
    let marked = lang.call_separators.iter().fold(written.to_owned(), |text, sep| text.replace(sep.as_str(), &PATH.to_string()));
    let mut out = Vec::new();
    let mut current = String::new();
    for c in marked.chars().chain(std::iter::once(' ')) {
        if c.is_alphanumeric() || c == '_' {
            current.push(c);
        } else if c == PATH {
            current.clear(); // a path prefix: the next segment replaces it
        } else {
            if !current.is_empty() && !lang.type_noise.contains(&current) {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
        }
    }
    out
}

/// `Ok` when `written` can carry `schema`, `Err(why)` when it can't, `None`
/// when the schema says nothing checkable (open, or a shape this table
/// can't judge).
pub fn carries(lang: &LanguageSpec, written: &str, schema: &Value) -> Option<Result<(), String>> {
    let mut words = words(written, lang);
    // Strip optional wrappers; remember the value may be absent.
    let mut nullable = false;
    while words.len() > 1 && lang.nullable_wrappers.contains(&words[0]) {
        words.remove(0);
        nullable = true;
    }
    let head = words.first()?.clone();
    let (schema, schema_nullable) = unwrap_null(schema);
    if nullable && !schema_nullable {
        return Some(Err(format!("{written} may be absent but a value is always expected")));
    }
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let title = arch_graph::schema::ref_title(r);
        return Some(if head == title { Ok(()) } else { Err(format!("{written} is not {title}")) });
    }
    let ty = schema.get("type").and_then(Value::as_str)?;
    let in_table = |t: &str| lang.schema_types.get(t).is_some_and(|names| names.contains(&head));
    let known = lang.schema_types.values().any(|names| names.contains(&head));
    let ok = in_table(ty) || (ty == "number" && in_table("integer"));
    Some(if ok || (ty == "object" && !known) {
        // A named type the table doesn't know can still be an object.
        Ok(())
    } else if !known {
        Err(format!("{written} is a named type where {ty} is expected"))
    } else {
        Err(format!("{written} doesn't carry {ty}"))
    })
}

/// The schema without a `null` option, and whether it had one.
fn unwrap_null(schema: &Value) -> (&Value, bool) {
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        let non_null: Vec<&Value> = options.iter().filter(|o| o.get("type").and_then(Value::as_str) != Some("null")).collect();
        if non_null.len() == 1 && non_null.len() < options.len() {
            return (non_null[0], true);
        }
    }
    (schema, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PackSet;
    use serde_json::json;

    #[test]
    fn written_types_are_judged_from_pack_tables() {
        let packs = PackSet::built_in().unwrap();
        let rust = &packs.by_name("rust").unwrap().spec.language;
        assert_eq!(carries(rust, "&str", &json!({"type": "string"})), Some(Ok(())));
        assert_eq!(carries(rust, "&mut Vec<u8>", &json!({"type": "array"})), Some(Ok(())));
        assert_eq!(carries(rust, "u32", &json!({"type": "number"})), Some(Ok(())), "integers are numbers");
        assert!(carries(rust, "f64", &json!({"type": "integer"})).unwrap().is_err());
        assert_eq!(carries(rust, "crate::model::User", &json!({"$ref": "User"})), Some(Ok(())));
        assert!(carries(rust, "Order", &json!({"$ref": "User"})).unwrap().is_err());
        assert_eq!(carries(rust, "Option<u32>", &json!({"anyOf": [{"type": "integer"}, {"type": "null"}]})), Some(Ok(())));
        assert!(carries(rust, "Option<u32>", &json!({"type": "integer"})).unwrap().is_err(), "may be absent");
        assert_eq!(carries(rust, "Config", &json!({"type": "object"})), Some(Ok(())));
        assert!(carries(rust, "Config", &json!({"type": "string"})).unwrap().is_err());
        assert_eq!(carries(rust, "u32", &json!({})), None, "open schema");
        let py = &packs.by_name("python").unwrap().spec.language;
        assert_eq!(carries(py, "Optional[int]", &json!({"anyOf": [{"type": "integer"}, {"type": "null"}]})), Some(Ok(())));
        assert_eq!(carries(py, "list[str]", &json!({"type": "array"})), Some(Ok(())));
    }
}
