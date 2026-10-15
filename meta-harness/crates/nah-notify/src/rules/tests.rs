use super::*;
use harness_contracts::{AuthState, DriverKind, Failure, ProviderSnapshot, ProviderStatus, RequestId, RuntimeRequest, Thread, Turn};

pub(crate) struct Fixed {
    pub lead_queued: bool,
    pub ended: Vec<ThreadId>,
}

impl World for Fixed {
    fn slot_of(&self, thread: &ThreadId) -> Option<SlotRef> {
        match thread.as_str() {
            "lead-t" => Some(SlotRef { id: SlotId::new("lead"), name: "lead".into(), lead: true }),
            "sub-t" => Some(SlotRef { id: SlotId::new("sub-1"), name: "reviewer".into(), lead: false }),
            _ => None,
        }
    }
    fn in_use(&self, instance: &InstanceId) -> bool {
        instance.as_str() == "claude-code"
    }
    fn closed_on_purpose(&self, thread: &ThreadId) -> bool {
        self.ended.contains(thread)
    }
    fn queued(&self, _: &ThreadId) -> bool {
        self.lead_queued
    }
    fn was_running(&self, turn: &TurnId) -> bool {
        turn.as_str() != "replayed"
    }
    fn reply(&self, _: &ThreadId, _: &TurnId) -> String {
        "All tests pass.".into()
    }
    fn sign_ins(&self, _: &InstanceId) -> u32 {
        2
    }
    fn now_ms(&self) -> u64 {
        7
    }
}

fn world() -> Fixed {
    Fixed { lead_queued: false, ended: Vec::new() }
}

pub(crate) fn request(id: &str, thread: &str, kind: RequestKind, status: RequestStatus) -> ClientEvent {
    let request = RuntimeRequest {
        id: RequestId::new(id),
        thread_id: ThreadId::new(thread),
        turn_id: None,
        item_id: None,
        kind,
        status,
        title: "Run cargo test".into(),
        detail: None,
        options: Vec::new(),
        questions: Vec::new(),
        response: None,
    };
    ClientEvent::Session { event: AdapterEvent::RequestUpdated { request } }
}

pub(crate) fn turn(id: &str, thread: &str, status: TurnStatus) -> ClientEvent {
    let failure = (status == TurnStatus::Failed).then(|| Failure { message: "the harness exited".into(), retryable: false });
    ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new(id), thread_id: ThreadId::new(thread), ordinal: 1, status, failure, usage: None } } }
}

fn instance(id: &str, status: AuthStatus) -> ClientEvent {
    let snapshot = ProviderSnapshot {
        instance_id: InstanceId::new(id),
        driver: DriverKind::new("acp"),
        display_name: "Claude Code".into(),
        status: ProviderStatus::Ready,
        auth: AuthState { status, account: None, methods: Vec::new() },
        capabilities: Default::default(),
        models: Vec::new(),
        commands: Vec::new(),
        skills: Vec::new(),
        runtime_modes: Vec::new(),
        supports_plan_mode: false,
        tool_servers: Vec::new(),
        problems: Vec::new(),
    };
    ClientEvent::InstanceUpdated { snapshot }
}

fn opened(changes: &[Change]) -> Vec<(String, NotificationKind, Option<String>, String)> {
    changes
        .iter()
        .filter_map(|c| match c {
            Change::Open(n) => Some((n.id.to_string(), n.kind, n.slot.as_ref().map(|s| s.to_string()), n.title.clone())),
            Change::End { .. } => None,
        })
        .collect()
}

fn with(notification: &Change) -> BTreeMap<NotificationId, Notification> {
    let Change::Open(n) = notification else { panic!("an open one") };
    BTreeMap::from([(n.id.clone(), n.clone())])
}

#[test]
fn a_pending_request_waits_on_the_user_until_it_is_answered() {
    let none = BTreeMap::new();
    for (kind, as_) in [(RequestKind::Approval { action: harness_contracts::ToolAction::Tool }, NotificationKind::Approval), (RequestKind::Approval { action: harness_contracts::ToolAction::FileChange }, NotificationKind::Approval), (RequestKind::Question, NotificationKind::Question), (RequestKind::Link, NotificationKind::Link)] {
        let changes = derive(&request("r1", "sub-t", kind, RequestStatus::Pending), &world(), &none);
        assert_eq!(opened(&changes), [("req:r1".to_owned(), as_, Some("sub-1".to_owned()), "Run cargo test".to_owned())]);
        let Change::Open(n) = &changes[0] else { unreachable!() };
        assert_eq!((n.level, n.request.as_ref().map(|r| r.as_str())), (harness_contracts::Level::Waiting, Some("r1")));
    }
    let open = with(&derive(&request("r1", "sub-t", RequestKind::Approval { action: harness_contracts::ToolAction::Command }, RequestStatus::Pending), &world(), &none)[0]);
    let answered = derive(&request("r1", "sub-t", RequestKind::Approval { action: harness_contracts::ToolAction::Command }, RequestStatus::Resolved), &world(), &open);
    assert_eq!(answered, [Change::End { id: NotificationId::new("req:r1"), to: NotificationState::Done }]);
    assert!(derive(&request("r2", "sub-t", RequestKind::Approval { action: harness_contracts::ToolAction::Command }, RequestStatus::Cancelled), &world(), &open).is_empty(), "nothing open for it: nothing to end");
}

#[test]
fn turns_that_end_are_updates_and_the_lead_going_quiet_says_so() {
    let none = BTreeMap::new();
    assert_eq!(opened(&derive(&turn("u1", "sub-t", TurnStatus::Completed), &world(), &none)), [("turn:u1".into(), NotificationKind::TurnFinished, Some("sub-1".into()), "reviewer finished a turn".into())]);
    let Change::Open(n) = &derive(&turn("u1", "sub-t", TurnStatus::Completed), &world(), &none)[0] else { unreachable!() };
    assert_eq!(n.detail.as_deref(), Some("All tests pass."), "what it said");
    assert_eq!(opened(&derive(&turn("u2", "lead-t", TurnStatus::Completed), &world(), &none))[0].3, "The lead is idle");
    let busy = Fixed { lead_queued: true, ..world() };
    assert_eq!(opened(&derive(&turn("u2", "lead-t", TurnStatus::Completed), &busy, &none))[0].3, "lead finished a turn", "more is queued: not idle");
    let failed = derive(&turn("u3", "sub-t", TurnStatus::Failed), &world(), &none);
    assert_eq!(opened(&failed)[0].1, NotificationKind::TurnFailed);
    let Change::Open(n) = &failed[0] else { unreachable!() };
    assert_eq!(n.detail.as_deref(), Some("the harness exited"));
    for quiet in [TurnStatus::Running, TurnStatus::Queued, TurnStatus::Interrupted, TurnStatus::Cancelled] {
        assert!(derive(&turn("u4", "sub-t", quiet), &world(), &none).is_empty(), "{quiet:?}");
    }
    assert!(derive(&turn("u5", "nobodys", TurnStatus::Completed), &world(), &none).is_empty(), "no slot runs it");
    assert!(derive(&turn("replayed", "sub-t", TurnStatus::Completed), &world(), &none).is_empty(), "history a resumed session replays isn't news");
}

#[test]
fn a_session_that_stops_unasked_is_a_crash_and_one_ended_on_purpose_isnt() {
    let none = BTreeMap::new();
    let closed = ClientEvent::ThreadClosed { thread_id: ThreadId::new("sub-t") };
    assert_eq!(opened(&derive(&closed, &world(), &none)), [("crash:sub-t".into(), NotificationKind::SessionCrashed, Some("sub-1".into()), "reviewer's session stopped".into())]);
    let asked = Fixed { ended: vec![ThreadId::new("sub-t")], ..world() };
    assert!(derive(&closed, &asked, &none).is_empty());
    let thread = Thread {
        id: ThreadId::new("lead-t"),
        instance_id: InstanceId::new("claude-code"),
        native_id: None,
        title: None,
        status: harness_contracts::ThreadStatus::Error,
        policy: Default::default(),
        model: None,
        controls: Default::default(),
        mcp_servers: Vec::new(),
    };
    let errored = ClientEvent::Session { event: AdapterEvent::ThreadUpdated { thread } };
    assert_eq!(opened(&derive(&errored, &world(), &none))[0].0, "crash:lead-t");
}

#[test]
fn a_harness_in_use_signed_out_waits_for_sign_in_numbered_each_time() {
    let none = BTreeMap::new();
    let out = derive(&instance("claude-code", AuthStatus::SignedOut), &world(), &none);
    assert_eq!(opened(&out), [("signin:claude-code:3".into(), NotificationKind::SignIn, None, "Claude Code needs signing in".into())]);
    assert!(derive(&instance("claude-code", AuthStatus::SignedOut), &world(), &with(&out[0])).is_empty(), "one at a time");
    assert!(derive(&instance("antigravity", AuthStatus::SignedOut), &world(), &none).is_empty(), "no slot uses it");
    assert_eq!(derive(&instance("claude-code", AuthStatus::SignedIn), &world(), &with(&out[0])), [Change::End { id: NotificationId::new("signin:claude-code:3"), to: NotificationState::Done }]);
}

#[test]
fn a_message_names_who_said_it_and_keeps_all_of_it() {
    let from = SlotRef { id: SlotId::new("sub-1"), name: "reviewer".into(), lead: false };
    let long = "x".repeat(2000);
    let n = message(Some(&from), &long, &world());
    assert_eq!((n.kind, n.title.as_str(), n.detail.as_ref().map(String::len)), (NotificationKind::Message, "From reviewer", Some(2000)));
    assert!(n.id.as_str().starts_with("msg:"));
}
