//! `{name}` placeholders in pack strings: which names a string uses, and
//! filling them in. A `{` that doesn't start a placeholder is an error, so a
//! typo is caught when the pack is loaded.

use std::collections::{BTreeMap, BTreeSet};

/// The placeholder names in `text`, in order.
pub fn names(text: &str) -> Vec<&str> {
    parse(text).map(|parts| parts.into_iter().filter_map(|p| p.err()).collect()).unwrap_or_default()
}

/// Fails when `text` uses a name outside `allowed`, or a stray brace.
pub fn check(text: &str, allowed: &BTreeSet<&str>) -> Result<(), String> {
    for part in parse(text)? {
        if let Err(name) = part
            && !allowed.contains(name)
        {
            return Err(format!("unknown placeholder {{{name}}} in '{text}'"));
        }
    }
    Ok(())
}

/// `text` with every placeholder filled from `values`.
pub fn expand(text: &str, values: &BTreeMap<&str, &str>) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    for part in parse(text)? {
        match part {
            Ok(literal) => out.push_str(literal),
            Err(name) => out.push_str(values.get(name).ok_or_else(|| format!("unknown placeholder {{{name}}} in '{text}'"))?),
        }
    }
    Ok(out)
}

/// Literal pieces (Ok) and placeholder names (Err).
fn parse(text: &str) -> Result<Vec<Result<&str, &str>>, String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push(Ok(&rest[..open]));
        let after = &rest[open + 1..];
        let close = after.find('}').ok_or_else(|| format!("a '{{' without '}}' in '{text}'"))?;
        let name = &after[..close];
        let valid = name.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_') && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !valid {
            return Err(format!("'{{{name}}}' isn't a placeholder in '{text}'"));
        }
        out.push(Err(name));
        rest = &after[close + 1..];
    }
    if rest.contains('}') {
        return Err(format!("a '}}' without '{{' in '{text}'"));
    }
    out.push(Ok(rest));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_found_checked_and_filled() {
        assert_eq!(names("{root}/x-{out}"), ["root", "out"]);
        let allowed: BTreeSet<&str> = ["root"].into_iter().collect();
        assert!(check("{root}", &allowed).is_ok());
        assert!(check("{rot}", &allowed).unwrap_err().contains("unknown placeholder {rot}"));
        assert!(check("a{b", &allowed).is_err() && check("a}b", &allowed).is_err() && check("{A}", &allowed).is_err());
        let values: BTreeMap<&str, &str> = [("os", "linux"), ("arch", "arm64")].into_iter().collect();
        assert_eq!(expand("x-{os}-{arch}.zip", &values).unwrap(), "x-linux-arm64.zip");
    }
}
