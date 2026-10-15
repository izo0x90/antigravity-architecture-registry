//! A request's answers as actions, with their keys: the same in the chat
//! and the inbox.

use harness_contracts::{ApprovalDecision, RequestStatus, RuntimeRequest};

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
}

/// The ways to answer `request` (pending), in its order. The chat and the
/// inbox both offer them so: the same keys wherever it's answered.
pub fn answers(request: &RuntimeRequest) -> Vec<Answer> {
    if request.status != RequestStatus::Pending {
        return Vec::new();
    }
    // A question's answers are never one kind's key: there are several.
    let alone = |d: ApprovalDecision| d != ApprovalDecision::Answer && request.options.iter().filter(|o| o.decision == d).count() == 1;
    request
        .options
        .iter()
        .map(|o| Answer {
            id: format!("request.{}.{}", request.id, o.id),
            option: o.id.clone(),
            label: format!("{}: {}", o.label, request.title),
            alias: alone(o.decision).then(|| format!("request.{}", crate::chat::actions::wire_name(&o.decision))),
            warning: o.warning.clone(),
        })
        .collect()
}
