//! A tree request as a URL's query (the views' endpoints are GETs): one
//! parameter per field, the lists as JSON. The cockpit writes it, the
//! server reads it, and both use this.

use super::{Opened, TreeRequest, ViewKind, Way};
use crate::Source;

/// What the asker adds to a request: the card to expand or place, and how
/// many levels.
pub const EXTRA: [&str; 2] = ["card", "levels"];

impl TreeRequest {
    /// The request as (name, value) pairs, not yet percent-encoded.
    pub fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![
            ("view", self.view.name().to_owned()),
            ("source", String::from(self.source.clone())),
            ("way", way_name(self.way).to_owned()),
        ];
        if let Some(root) = &self.root {
            out.push(("root", root.clone()));
        }
        if let Some(depth) = self.depth {
            out.push(("depth", depth.to_string()));
        }
        if !self.opened.is_empty() {
            out.push(("opened", serde_json::to_string(&self.opened).unwrap_or_default()));
        }
        if !self.closed.is_empty() {
            out.push(("closed", serde_json::to_string(&self.closed).unwrap_or_default()));
        }
        if self.full_code {
            out.push(("full_code", "true".to_owned()));
        }
        if let Some(only) = &self.only {
            out.push(("only", serde_json::to_string(only).unwrap_or_default()));
        }
        out
    }

    /// The request a query's (decoded) pairs ask for; names in `extra` are
    /// left for the caller, any other name is an error.
    pub fn from_query_pairs(pairs: &[(String, String)], extra: &[&str]) -> Result<Self, String> {
        let mut request = TreeRequest::default();
        for (name, value) in pairs {
            let bad = |what: &str| format!("{name}={value}: {what}");
            match name.as_str() {
                "view" => request.view = ViewKind::from_name(value).ok_or_else(|| bad("use contains, calls or data_flow"))?,
                "source" => request.source = value.parse::<Source>()?,
                "way" => {
                    request.way = match value.as_str() {
                        "down" => Way::Down,
                        "up" => Way::Up,
                        _ => return Err(bad("use down or up")),
                    }
                }
                "root" => request.root = Some(value.clone()),
                "depth" => request.depth = Some(value.parse().map_err(|_| bad("a number of levels"))?),
                "opened" => request.opened = serde_json::from_str::<Vec<Opened>>(value).map_err(|e| bad(&e.to_string()))?,
                "closed" => request.closed = serde_json::from_str::<Vec<String>>(value).map_err(|e| bad(&e.to_string()))?,
                "full_code" => request.full_code = value.parse().map_err(|_| bad("true or false"))?,
                "only" => request.only = Some(serde_json::from_str::<Vec<String>>(value).map_err(|e| bad(&e.to_string()))?),
                other if extra.contains(&other) => {}
                _ => return Err(format!("{name}: not something a view is asked with")),
            }
        }
        Ok(request)
    }
}

fn way_name(way: Way) -> &'static str {
    match way {
        Way::Down => "down",
        Way::Up => "up",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlanId;

    fn decoded(pairs: Vec<(&'static str, String)>) -> Vec<(String, String)> {
        pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
    }

    #[test]
    fn a_request_goes_through_a_query_and_comes_back_the_same() {
        let request = TreeRequest {
            view: ViewKind::Calls,
            source: Source::Plan(PlanId::new("p").unwrap()),
            root: Some("rust:a::run".into()),
            way: Way::Up,
            depth: Some(3),
            opened: vec![Opened { card: "type:Vec<A, B>".into(), levels: 2 }],
            closed: vec!["rust:a::log".into()],
            full_code: true,
            only: Some(vec!["rust:a::run".into()]),
        };
        assert_eq!(TreeRequest::from_query_pairs(&decoded(request.query_pairs()), &[]).unwrap(), request);
        assert_eq!(TreeRequest::from_query_pairs(&[], &[]).unwrap(), TreeRequest::default());
        let mut asked = decoded(request.query_pairs());
        asked.push(("card".into(), "rust:a::step".into()));
        assert!(TreeRequest::from_query_pairs(&asked, &[]).unwrap_err().contains("card: not something"));
        assert_eq!(TreeRequest::from_query_pairs(&asked, &EXTRA).unwrap(), request);
        assert!(TreeRequest::from_query_pairs(&[("view".into(), "story".into())], &[]).unwrap_err().contains("use contains"));
    }
}
