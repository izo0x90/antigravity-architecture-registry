//! What a harness asks, from how it asked. ACP carries approvals as
//! permission requests and questions as forms or links; a harness may also
//! ask a question through a permission request (Google's Antigravity server
//! does: a made-up tool call `interaction_…`, every answer "allow once").
//! Its pack says so (`[provides.settings.questions]`); this reads that.
//! Anything it can't place is a question: shown to the user, never answered
//! for them.

use agent_client_protocol::schema::v1 as acp;
use harness_contracts::{ApprovalDecision, ApprovalOption, RequestKind, ToolAction, ToolKind};

use crate::settings::QuestionSettings;

/// What a tool of `kind` would do, asking for approval.
pub fn action(kind: &ToolKind) -> ToolAction {
    match kind {
        ToolKind::Execute => ToolAction::Command,
        ToolKind::Edit | ToolKind::Delete | ToolKind::Move => ToolAction::FileChange,
        ToolKind::Read => ToolAction::FileRead,
        _ => ToolAction::Tool,
    }
}

fn rejects(kind: &acp::PermissionOptionKind) -> bool {
    matches!(kind, acp::PermissionOptionKind::RejectOnce | acp::PermissionOptionKind::RejectAlways)
}

/// What a permission request about tool call `tool_id` (of `kind`) asks.
pub fn permission(settings: &QuestionSettings, tool_id: &str, kind: &ToolKind, options: &[acp::PermissionOption]) -> RequestKind {
    if settings.permission_tool_prefix.as_deref().is_some_and(|prefix| tool_id.starts_with(prefix)) {
        return RequestKind::Question;
    }
    // An approval is allow or deny: a deny among them, no two of one kind.
    let one_each = options.iter().enumerate().all(|(i, o)| options[..i].iter().all(|p| p.kind != o.kind));
    if options.iter().any(|o| rejects(&o.kind)) && one_each {
        return RequestKind::Approval { action: action(kind) };
    }
    tracing::warn!("a permission request with no deny among its options (tool call {tool_id}): shown as a question, never answered for the user");
    RequestKind::Question
}

/// Its options as the user answers them: allow or deny for an approval;
/// for a question, its answers (a deny among them stays one).
pub fn options(asked: RequestKind, native: &[acp::PermissionOption]) -> Vec<ApprovalOption> {
    native
        .iter()
        .map(|o| {
            let decision = match (asked, &o.kind) {
                (_, kind) if rejects(kind) => ApprovalDecision::Decline,
                (RequestKind::Question | RequestKind::Link, _) => ApprovalDecision::Answer,
                (RequestKind::Approval { .. }, acp::PermissionOptionKind::AllowAlways) => ApprovalDecision::AcceptForSession,
                (RequestKind::Approval { .. }, _) => ApprovalDecision::Accept,
            };
            ApprovalOption { id: o.option_id.to_string(), decision, label: o.name.clone(), warning: None }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str, kind: acp::PermissionOptionKind) -> acp::PermissionOption {
        acp::PermissionOption::new(id.to_owned(), id.to_owned(), kind)
    }

    #[test]
    fn an_allow_or_deny_set_is_an_approval_and_anything_else_a_question() {
        use acp::PermissionOptionKind as K;
        let none = QuestionSettings::default();
        let approval = [option("allow", K::AllowOnce), option("always", K::AllowAlways), option("deny", K::RejectOnce)];
        assert_eq!(permission(&none, "t1", &ToolKind::Execute, &approval), RequestKind::Approval { action: ToolAction::Command });
        assert_eq!(permission(&none, "t1", &ToolKind::Edit, &approval), RequestKind::Approval { action: ToolAction::FileChange });
        assert_eq!(permission(&none, "t1", &ToolKind::Other, &approval), RequestKind::Approval { action: ToolAction::Tool });
        let answers = [option("1", K::AllowOnce), option("2", K::AllowOnce)];
        assert_eq!(permission(&none, "t1", &ToolKind::Other, &answers), RequestKind::Question, "no deny, two of a kind: answers");
        assert_eq!(permission(&none, "t1", &ToolKind::Other, &[option("ok", K::AllowOnce)]), RequestKind::Question, "no way to say no: not an approval");
        // A harness that says how its questions come: by their tool call.
        let antigravity = QuestionSettings { permission_tool_prefix: Some("interaction_".into()), ..Default::default() };
        let trust = [option("trust", K::AllowOnce), option("dont_trust", K::RejectOnce)];
        assert_eq!(permission(&antigravity, "interaction_1a2b", &ToolKind::Other, &trust), RequestKind::Question, "a question even with a deny among its answers");
        assert_eq!(permission(&antigravity, "call_9", &ToolKind::Execute, &approval), RequestKind::Approval { action: ToolAction::Command }, "its real tool calls stay approvals");
        let decisions: Vec<ApprovalDecision> = options(RequestKind::Question, &trust).into_iter().map(|o| o.decision).collect();
        assert_eq!(decisions, [ApprovalDecision::Answer, ApprovalDecision::Decline]);
    }
}
