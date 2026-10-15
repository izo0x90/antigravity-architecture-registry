//! A request's answers as actions, with their keys: the same in the chat
//! and the inbox.

use harness_contracts::{ApprovalDecision, RequestKind, RequestStatus, RuntimeRequest};

use crate::actions::{Action, Style, Tone};

/// One way to answer a request, as an action offers it.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    /// The action's id: `request.<request>.<option>`.
    pub id: String,
    pub option: String,
    /// `"Allow: run cargo test"`.
    pub label: String,
    /// `request.accept` and the like, which the keys bind (`y`, `d`…): only
    /// when its kind is the only one of it. Several of a kind (a question's
    /// answers) are numbered under the leader instead.
    pub alias: Option<String>,
    pub warning: Option<String>,
    /// As its control says it: `"Allow"`, `"Redis"`.
    pub caption: String,
    /// A question's answers are a list to pick from; anything else (allow
    /// or deny, a form's submit or skip, done) a row of buttons.
    pub style: Style,
    /// Allow reads green, deny red, an answer plain.
    pub tone: Tone,
}

impl Answer {
    /// `action` (this answer's) as it shows: under the request, while the
    /// cursor is on it (`here`).
    pub fn place(&self, action: Action, here: bool) -> Action {
        let mut action = action;
        if let Some(alias) = &self.alias {
            action = action.alias(alias.clone());
        }
        if let Some(w) = &self.warning {
            action = action.detail(w.clone());
        }
        if !here {
            return action;
        }
        let mut action = action.on_item(self.style, self.caption.clone(), None);
        if let Some(control) = action.control.as_mut() {
            control.tone = Some(self.tone);
        }
        action
    }
}

/// The ways to answer `request` (pending), in its order. The chat and the
/// inbox both offer them so: the same keys wherever it's answered.
pub fn answers(request: &RuntimeRequest) -> Vec<Answer> {
    if request.status != RequestStatus::Pending {
        return Vec::new();
    }
    // A question's answers are never one kind's key: there are several.
    let alone = |d: ApprovalDecision| d != ApprovalDecision::Answer && request.options.iter().filter(|o| o.decision == d).count() == 1;
    let style = if request.kind == RequestKind::Question && request.questions.is_empty() { Style::Choice } else { Style::Button };
    let tone = |d: ApprovalDecision| match d {
        ApprovalDecision::Decline | ApprovalDecision::Cancel => Tone::Error,
        ApprovalDecision::Answer => Tone::Plain,
        _ => Tone::Ok,
    };
    request
        .options
        .iter()
        .map(|o| Answer {
            id: format!("request.{}.{}", request.id, o.id),
            option: o.id.clone(),
            label: format!("{}: {}", o.label, request.title),
            alias: alone(o.decision).then(|| format!("request.{}", crate::chat::actions::wire_name(&o.decision))),
            warning: o.warning.clone(),
            caption: o.label.clone(),
            style,
            tone: tone(o.decision),
        })
        .collect()
}
