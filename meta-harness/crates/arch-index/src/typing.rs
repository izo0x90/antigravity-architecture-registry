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
        if lang.is_identifier_char(c) {
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
    // `string | null`, `str | None`: the value may be absent; judge the rest.
    let alternatives = split_union(written, lang);
    let (nulls, values): (Vec<&str>, Vec<&str>) = alternatives.iter().partition(|a| lang.null_types.iter().any(|n| n == a.trim()));
    if !nulls.is_empty() && values.len() == 1 {
        let (inner, schema_nullable) = unwrap_null(schema);
        if !schema_nullable {
            return Some(Err(format!("{written} may be absent but a value is always expected")));
        }
        return carries(lang, values[0].trim(), inner);
    }
    // `string[]` / `[]string`: an array whatever the item type.
    let bare = lang.strip_references(written);
    if lang.array_suffixes.iter().any(|s| bare.ends_with(s.as_str())) || lang.array_prefixes.iter().any(|p| bare.starts_with(p.as_str())) {
        let (schema, _) = unwrap_null(schema);
        let ty = schema.get("type").and_then(Value::as_str)?;
        return Some(if ty == "array" { Ok(()) } else { Err(format!("{written} is an array where {ty} is expected")) });
    }
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

/// A written type's union alternatives (`a | b`), outside brackets.
fn split_union<'a>(written: &'a str, lang: &LanguageSpec) -> Vec<&'a str> {
    if lang.union_separators.is_empty() {
        return vec![written];
    }
    let mut out = Vec::new();
    let (mut depth, mut start, mut skip_until) = (0i32, 0usize, 0usize);
    for (i, c) in written.char_indices() {
        if i < skip_until {
            continue;
        }
        match c {
            '(' | '[' | '<' | '{' => depth += 1,
            ')' | ']' | '>' | '}' => depth -= 1,
            _ if depth == 0 => {
                if let Some(sep) = lang.union_separators.iter().find(|sep| written[i..].starts_with(sep.as_str())) {
                    out.push(&written[start..i]);
                    skip_until = i + sep.len();
                    start = skip_until;
                }
            }
            _ => {}
        }
    }
    out.push(&written[start..]);
    out
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
    use serde_json::json;

    #[test]
    fn written_types_are_judged_from_pack_tables() {
        let packs = crate::PackSet::from_shipped_dir(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")).unwrap();
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
        // Unions with a null type: may be absent.
        let nullable_int = json!({"anyOf": [{"type": "integer"}, {"type": "null"}]});
        assert_eq!(carries(py, "int | None", &nullable_int), Some(Ok(())));
        assert!(carries(py, "int | None", &json!({"type": "integer"})).unwrap().is_err(), "may be absent");
        let ts = &packs.by_name("typescript").unwrap().spec.language;
        let nullable_str = json!({"anyOf": [{"type": "string"}, {"type": "null"}]});
        assert_eq!(carries(ts, "string | null", &nullable_str), Some(Ok(())));
        assert_eq!(carries(ts, "string[] | undefined", &json!({"anyOf": [{"type": "array"}, {"type": "null"}]})), Some(Ok(())));
        assert!(carries(ts, "number | null", &nullable_str).unwrap().is_err());
        let ts = &packs.by_name("typescript").unwrap().spec.language;
        assert_eq!(carries(ts, "string[]", &json!({"type": "array"})), Some(Ok(())), "array suffix");
        assert!(carries(ts, "string[]", &json!({"type": "string"})).unwrap().is_err());
        assert_eq!(carries(ts, "string", &json!({"type": "string"})), Some(Ok(())));
    }
}
