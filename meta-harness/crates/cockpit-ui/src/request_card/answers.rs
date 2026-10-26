//! A request's answers as actions, with their keys: the same in the chat
//! and the inbox.

use harness_contracts::{ApprovalDecision, RequestId, RequestKind, RequestStatus, RuntimeRequest};

use crate::actions::{Action, Group, PRIMARY, Style, Tone};

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
    /// `action` (this answer's) as it shows: under the request, as `under`
    /// says.
    pub fn place(&self, action: Action, under: Under<'_>) -> Action {
        let mut action = action;
        if let Some(alias) = &self.alias {
            action = action.alias(alias.clone());
        }
        if let Some(w) = &self.warning {
            action = action.detail(w.clone());
        }
        let Some(at) = under.shown() else { return action };
        let mut action = action.on_waiting(self.style, self.caption.clone(), None);
        if let Some(control) = action.control.as_mut() {
            control.tone = Some(self.tone);
            control.under = at;
        }
        action
    }
}

/// The page a request asks you to open (a link's), while it's pending.
pub fn page(request: &RuntimeRequest) -> Option<String> {
    let url = request.detail.as_ref().filter(|d| d.starts_with("https://") || d.starts_with("http://"))?;
    (request.status == RequestStatus::Pending).then(|| url.clone())
}

/// `action` (opening `request`'s page) as it shows: under the request,
/// as `under` says.
pub fn place_page(action: Action, under: Under<'_>) -> Action {
    let Some(at) = under.shown() else { return action };
    let mut action = action.on_waiting(Style::Button, "Open the page", Some(egui_phosphor::regular::ARROW_SQUARE_OUT));
    if let Some(control) = action.control.as_mut() {
        control.under = at;
    }
    action
}

/// Where a request's answers show as controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Under<'a> {
    /// Not shown: its keys and the action list only.
    Nowhere,
    /// Under what you're on.
    Here,
    /// Under the view's place `key` (its card).
    Place(&'a str),
}

impl Under<'_> {
    /// Shown: under which place (none: what you're on).
    fn shown(self) -> Option<Option<String>> {
        match self {
            Self::Nowhere => None,
            Self::Here => Some(None),
            Self::Place(key) => Some(Some(key.to_owned())),
        }
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
            alias: alone(o.decision).then(|| format!("request.{}", crate::actions::wire_name(&o.decision))),
            warning: o.warning.clone(),
            caption: o.label.clone(),
            style,
            tone: tone(o.decision),
        })
        .collect()
}

/// Where a request's answers are offered: under it (you're on it), and
/// whether they hold the keys (`y`, `d`…: one request's at a time).
#[derive(Clone, Copy, Debug)]
pub struct Offer<'a> {
    pub under: Under<'a>,
    pub keyed: bool,
}

/// Of several waiting, the one whose answers hold the keys: the one you're
/// on (`on`), else the first.
pub fn keyed<'a>(waiting: impl IntoIterator<Item = &'a RuntimeRequest>, on: Option<&RequestId>) -> Option<RequestId> {
    let waiting: Vec<&RuntimeRequest> = waiting.into_iter().collect();
    waiting.iter().find(|r| Some(&r.id) == on).or(waiting.first()).map(|r| r.id.clone())
}

/// A waiting request's actions, the same wherever it's answered (the chat,
/// the inbox): opening its page, when it has one, and each answer, placed
/// as `at` says. `page` and `answer` are the caller's commands for them (an
/// answer by its option's id).
pub fn offer<C: 'static>(request: &RuntimeRequest, at: Offer<'_>, page: impl FnOnce(String) -> C, answer: impl Fn(String) -> C) -> Vec<Action> {
    let act = |id: &str, label: &str, detail: &str, command: C| Action::new(id, label, detail, command).focused().group(Group::Act);
    let mut out = Vec::new();
    if let Some(url) = self::page(request) {
        out.push(place_page(act(&format!("request.{}.page", request.id), &format!("Open the page: {}", request.title), "The page the agent asks you to open, in a new tab", page(url)), at.under));
    }
    for one in answers(request) {
        let one = if at.keyed { one } else { Answer { alias: None, ..one } };
        out.push(one.place(act(&one.id, &one.label, "Answers the agent's request this way", answer(one.option.clone())).importance(PRIMARY), at.under));
    }
    out
}
