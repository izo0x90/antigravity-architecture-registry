//! Questions an agent asks (ACP `elicitation/create`) as the user's
//! questions, and their answers back as the form's content. A form field
//! with choices is a question to pick from; one without, a question to
//! type an answer to. An agent may give a question its own answer box
//! (a field named the question's plus a suffix): that's the question's
//! "type your own" answer, not a question of its own.

use std::collections::BTreeMap;

use agent_client_protocol::schema::v1 as acp;
use harness_contracts::{Question, QuestionOption};

/// The questions of one form, and how each answer goes back.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub questions: Vec<Question>,
    fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq)]
struct Field {
    key: String,
    kind: Kind,
    /// Its own answer box's key.
    custom: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    One,
    Many,
    Text,
    Yes,
    Integer,
    Number,
}

impl Form {
    pub fn new(schema: &acp::ElicitationSchema, message: &str, custom_suffix: Option<&str>) -> Self {
        let required = schema.required.clone().unwrap_or_default();
        let is_box = |key: &str| custom_suffix.and_then(|s| key.strip_suffix(s)).is_some_and(|q| schema.properties.contains_key(q));
        let single = schema.properties.keys().filter(|k| !is_box(k)).count() == 1;
        let (mut questions, mut fields) = (Vec::new(), Vec::new());
        for (key, property) in schema.properties.iter().filter(|(k, _)| !is_box(k)) {
            let custom = custom_suffix.map(|s| format!("{key}{s}")).filter(|k| schema.properties.contains_key(k));
            let (kind, title, description, options) = describe(property);
            let text = description.clone().or_else(|| title.clone()).unwrap_or_else(|| key.clone());
            questions.push(Question {
                id: key.clone(),
                header: title.unwrap_or_else(|| key.clone()),
                // A form of one question asks it in its message.
                question: if single && !message.is_empty() { message.to_owned() } else { text },
                allow_custom_answer: custom.is_some() || options.is_empty(),
                multi_select: kind == Kind::Many,
                required: required.contains(key),
                options,
            });
            fields.push(Field { key: key.clone(), kind, custom });
        }
        Self { questions, fields }
    }

    /// The form's content for `answers` (by question): a pick goes in its
    /// field, an answer that's none of the choices in its answer box.
    pub fn content(&self, answers: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, acp::ElicitationContentValue> {
        let mut content = BTreeMap::new();
        for (field, question) in self.fields.iter().zip(&self.questions) {
            let Some(given) = answers.get(&field.key).filter(|a| !a.is_empty()) else { continue };
            let is_choice = |a: &String| question.options.iter().any(|o| o.value.as_ref().unwrap_or(&o.label) == a);
            let (picked, typed): (Vec<String>, Vec<String>) =
                given.iter().cloned().partition(|a| question.options.is_empty() || is_choice(a));
            let value = match field.kind {
                Kind::Many if !picked.is_empty() => Some(acp::ElicitationContentValue::StringArray(picked)),
                Kind::One | Kind::Text => picked.into_iter().next().map(acp::ElicitationContentValue::String),
                Kind::Yes => picked.first().map(|a| acp::ElicitationContentValue::Boolean(a == "true")),
                Kind::Integer => picked.first().and_then(|a| a.trim().parse().ok()).map(acp::ElicitationContentValue::Integer),
                Kind::Number => picked.first().and_then(|a| a.trim().parse().ok()).map(acp::ElicitationContentValue::Number),
                Kind::Many => None,
            };
            if let Some(value) = value {
                content.insert(field.key.clone(), value);
            }
            match (&field.custom, typed.is_empty()) {
                (Some(custom), false) => {
                    content.insert(custom.clone(), acp::ElicitationContentValue::String(typed.join(", ")));
                }
                // No box of its own: a typed answer is the answer.
                (None, false) if field.kind != Kind::Many => {
                    content.entry(field.key.clone()).or_insert(acp::ElicitationContentValue::String(typed.join(", ")));
                }
                _ => {}
            }
        }
        content
    }
}

fn option(value: &str, title: &str, description: Option<&String>) -> QuestionOption {
    QuestionOption { label: title.to_owned(), description: description.cloned(), value: (value != title).then(|| value.to_owned()) }
}

fn plain(values: &[String]) -> Vec<QuestionOption> {
    values.iter().map(|v| option(v, v, None)).collect()
}

fn titled(options: &[acp::EnumOption]) -> Vec<QuestionOption> {
    options.iter().map(|o| option(&o.value, &o.title, o.description.as_ref())).collect()
}

/// A field's kind, title, description and choices.
fn describe(property: &acp::ElicitationPropertySchema) -> (Kind, Option<String>, Option<String>, Vec<QuestionOption>) {
    use acp::ElicitationPropertySchema as P;
    match property {
        P::String(s) => {
            let options = match (&s.one_of, &s.enum_values) {
                (Some(titled_options), _) => titled(titled_options),
                (None, Some(values)) => plain(values),
                (None, None) => Vec::new(),
            };
            let kind = if options.is_empty() { Kind::Text } else { Kind::One };
            (kind, s.title.clone(), s.description.clone(), options)
        }
        P::Array(a) => {
            let options = match &a.items {
                acp::MultiSelectItems::String(items) => plain(&items.values),
                acp::MultiSelectItems::Titled(items) => titled(&items.options),
                _ => Vec::new(),
            };
            (Kind::Many, a.title.clone(), a.description.clone(), options)
        }
        P::Boolean(b) => {
            (Kind::Yes, b.title.clone(), b.description.clone(), vec![option("true", "Yes", None), option("false", "No", None)])
        }
        P::Integer(i) => (Kind::Integer, i.title.clone(), i.description.clone(), Vec::new()),
        P::Number(n) => (Kind::Number, n.title.clone(), n.description.clone(), Vec::new()),
        _ => (Kind::Text, None, None, Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape Claude Code asks its questions in.
    fn claude_form() -> acp::ElicitationSchema {
        serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "question_0": { "type": "string", "title": "Store", "description": "Which store?", "oneOf": [
                    { "const": "Redis", "title": "Redis", "description": "In memory" },
                    { "const": "Postgres", "title": "Postgres" } ] },
                "question_0_custom": { "type": "string", "title": "Other" },
                "question_1": { "type": "array", "title": "Checks", "description": "Run which?", "items": { "anyOf": [
                    { "const": "lint", "title": "lint" }, { "const": "test", "title": "test" } ] } },
                "question_1_custom": { "type": "string", "title": "Other" }
            }
        }))
        .unwrap()
    }

    #[test]
    fn fields_become_questions_and_answer_boxes_fold_into_theirs() {
        let form = Form::new(&claude_form(), "Please answer the following questions.", Some("_custom"));
        let shape: Vec<_> = form
            .questions
            .iter()
            .map(|q| (q.id.as_str(), q.header.as_str(), q.question.as_str(), q.multi_select, q.allow_custom_answer, q.options.len()))
            .collect();
        assert_eq!(shape, [("question_0", "Store", "Which store?", false, true, 2), ("question_1", "Checks", "Run which?", true, true, 2)]);
        assert_eq!(form.questions[0].options[0].description.as_deref(), Some("In memory"));
    }

    #[test]
    fn picks_go_in_their_fields_and_typed_answers_in_their_boxes() {
        let form = Form::new(&claude_form(), "", Some("_custom"));
        let answers = BTreeMap::from([
            ("question_0".to_owned(), vec!["Valkey".to_owned()]),
            ("question_1".to_owned(), vec!["lint".to_owned(), "fmt".to_owned()]),
        ]);
        let content = form.content(&answers);
        assert_eq!(content.get("question_0"), None, "nothing picked");
        assert_eq!(content.get("question_0_custom"), Some(&acp::ElicitationContentValue::String("Valkey".into())));
        assert_eq!(content.get("question_1"), Some(&acp::ElicitationContentValue::StringArray(vec!["lint".into()])));
        assert_eq!(content.get("question_1_custom"), Some(&acp::ElicitationContentValue::String("fmt".into())));
    }

    #[test]
    fn plain_fields_take_their_own_kinds_of_answer() {
        let schema: acp::ElicitationSchema = serde_json::from_value(serde_json::json!({
            "type": "object", "required": ["name"],
            "properties": { "name": { "type": "string" }, "ok": { "type": "boolean", "title": "Go ahead?" }, "n": { "type": "integer" } }
        }))
        .unwrap();
        let form = Form::new(&schema, "Details", None);
        assert!(
            form.questions.iter().find(|q| q.id == "name").is_some_and(|q| q.required && q.allow_custom_answer && q.options.is_empty())
        );
        let answers = BTreeMap::from([
            ("name".to_owned(), vec!["x".to_owned()]),
            ("ok".to_owned(), vec!["true".to_owned()]),
            ("n".to_owned(), vec!["4".to_owned()]),
        ]);
        let content = form.content(&answers);
        assert_eq!(content.get("name"), Some(&acp::ElicitationContentValue::String("x".into())));
        assert_eq!(content.get("ok"), Some(&acp::ElicitationContentValue::Boolean(true)));
        assert_eq!(content.get("n"), Some(&acp::ElicitationContentValue::Integer(4)));
    }
}
