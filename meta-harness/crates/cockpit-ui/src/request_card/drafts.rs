//! What's picked and typed for a request's questions, until it's sent.

use std::collections::{BTreeMap, HashMap};

use harness_contracts::{ApprovalDecision, Question, RequestId, RequestResponse, RuntimeRequest};

/// What's picked and typed for each request's questions, by question.
#[derive(Default)]
pub struct Drafts {
    pub(super) drafts: HashMap<RequestId, Draft>,
}

#[derive(Default)]
pub(super) struct Draft {
    pub(super) picks: BTreeMap<String, Vec<String>>,
    pub(super) typed: BTreeMap<String, String>,
}

/// The value an option answers with.
pub(super) fn value(option: &harness_contracts::QuestionOption) -> &str {
    option.value.as_deref().unwrap_or(&option.label)
}

impl Drafts {
    /// The answers to `request`'s questions so far: the picks, then what
    /// was typed.
    pub fn answers(&self, request: &RuntimeRequest) -> BTreeMap<String, Vec<String>> {
        let Some(draft) = self.drafts.get(&request.id) else { return BTreeMap::new() };
        request
            .questions
            .iter()
            .filter_map(|q| {
                let mut answer = draft.picks.get(&q.id).cloned().unwrap_or_default();
                answer.extend(draft.typed.get(&q.id).map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()));
                (!answer.is_empty()).then(|| (q.id.clone(), answer))
            })
            .collect()
    }

    /// Every question that must be answered is.
    pub fn complete(&self, request: &RuntimeRequest) -> bool {
        let answers = self.answers(request);
        request.questions.iter().all(|q| !q.required || answers.contains_key(&q.id))
    }

    /// The response choosing `option`: with the answers when it accepts.
    pub fn response(&self, request: &RuntimeRequest, option: &str) -> RequestResponse {
        let accepts = request.options.iter().any(|o| o.id == option && matches!(o.decision, ApprovalDecision::Accept | ApprovalDecision::AcceptForSession | ApprovalDecision::AcceptAlways));
        RequestResponse { option_id: Some(option.to_owned()), answers: if accepts { self.answers(request) } else { BTreeMap::new() } }
    }

    pub(super) fn pick(&mut self, request: &RequestId, question: &Question, value: &str) {
        let picks = self.drafts.entry(request.clone()).or_default().picks.entry(question.id.clone()).or_default();
        match picks.iter().position(|p| p == value) {
            Some(at) => {
                picks.remove(at);
            }
            None if question.multi_select => picks.push(value.to_owned()),
            None => *picks = vec![value.to_owned()],
        }
    }

    pub(super) fn picked(&self, request: &RequestId, question: &str, value: &str) -> bool {
        self.drafts.get(request).and_then(|d| d.picks.get(question)).is_some_and(|p| p.iter().any(|v| v == value))
    }
}

