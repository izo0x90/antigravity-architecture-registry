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
    BTreeMap::from([(n.id.clone(), (**n).clone())])
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

#[test]
fn a_terminals_bell_end_and_long_commands_are_updates_about_it() {
    let terminal = harness_contracts::Terminal {
        id: harness_contracts::TerminalId::new("t1"),
        title: "make".into(),
        status: harness_contracts::TerminalStatus::Running,
        size: harness_contracts::TermSize { cols: 80, rows: 24 },
        shell: "/bin/bash".into(),
        folder: "/p".into(),
    };
    let news = |news| derive(&ClientEvent::TerminalNews { terminal: terminal.clone(), news }, &world(), &BTreeMap::new());
    let one = |changes: Vec<Change>| match changes.as_slice() {
        [Change::Open(n)] => (**n).clone(),
        other => panic!("one opened: {other:?}"),
    };
    let bell = one(news(TerminalNews::Bell));
    assert_eq!((bell.kind, bell.level, bell.terminal.as_ref().map(|t| t.as_str()), bell.title.as_str()), (NotificationKind::TerminalBell, harness_contracts::Level::Update, Some("t1"), "make rang its bell"));
    let failed = one(news(TerminalNews::Finished { code: Some(2), ran_ms: 75_000 }));
    assert_eq!((failed.kind, failed.detail.as_deref()), (NotificationKind::CommandFailed, Some("exit code 2, after 1 min 15 s")));
    assert!(failed.kind.is_failure());
    let done = one(news(TerminalNews::Finished { code: Some(0), ran_ms: 12_000 }));
    assert_eq!((done.kind, done.detail.as_deref()), (NotificationKind::CommandFinished, Some("after 12 s")));
    let ended = one(news(TerminalNews::Ended { code: Some(1) }));
    assert_eq!((ended.kind, ended.title.as_str()), (NotificationKind::ShellEnded, "make: its shell ended"));
    assert_ne!(bell.id, done.id, "each its own");
    let open = BTreeMap::from([(bell.id.clone(), bell.clone())]);
    assert!(derive(&ClientEvent::TerminalNews { terminal: terminal.clone(), news: TerminalNews::Bell }, &world(), &open).is_empty(), "a bell while one is open: nothing new");
}

#[test]
fn a_discussion_waits_on_the_user_while_its_their_move_and_is_done_once_it_isnt() {
    use harness_contracts::{Discussion, DiscussionId, DiscussionKind, DiscussionStatus, Named, Said, SaidKind, Subject};
    let said = |by: &str, kind, text: &str| Said { by: by.into(), at_ms: 0, kind, text: text.into(), changed: Vec::new() };
    let d = |status, by: &str, asked: &str, said: Vec<Said>| ClientEvent::DiscussionUpdated {
        discussion: Discussion {
            id: DiscussionId::new("d3"),
            on: Named { subject: Subject::Plan { plan: arch_graph::PlanId::new("p").unwrap() }, what: "plan p".into() },
            kept: None,
            kind: DiscussionKind::Question,
            by: by.into(),
            asked: asked.into(),
            status,
            said,
        },
    };
    let none = BTreeMap::new();
    assert!(derive(&d(DiscussionStatus::Open, "user", "lead", vec![said("user", SaidKind::Message, "Why?")]), &world(), &none).is_empty(), "the agent's move: nothing waits on the user");
    let answered = derive(&d(DiscussionStatus::Answered, "user", "lead", vec![said("user", SaidKind::Message, "Why?"), said("lead", SaidKind::Answer, "Because.")]), &world(), &none);
    assert_eq!(opened(&answered), [("disc:d3:2".to_owned(), NotificationKind::Discussion, None, "lead answered d3 on plan p: settle or reopen it".to_owned())]);
    let Change::Open(n) = &answered[0] else { unreachable!() };
    assert_eq!((n.level, n.discussion.as_ref().map(|d| d.as_str()), n.detail.as_deref()), (harness_contracts::Level::Waiting, Some("d3"), Some("Because.")));
    let open = with(&answered[0]);
    assert!(derive(&d(DiscussionStatus::Answered, "user", "lead", vec![said("user", SaidKind::Message, "Why?"), said("lead", SaidKind::Answer, "Because.")]), &world(), &open).is_empty(), "nothing new: the same one");
    let settled = derive(&d(DiscussionStatus::Settled, "user", "lead", vec![said("lead", SaidKind::Answer, "Because."), said("user", SaidKind::Settled, "")]), &world(), &open);
    assert_eq!(settled, [Change::End { id: NotificationId::new("disc:d3:2"), to: NotificationState::Done }]);
    // The user's move again later (reopened, answered again): a new one, the last one done.
    let again = derive(&d(DiscussionStatus::Answered, "user", "lead", vec![said("user", SaidKind::Message, "Why?"), said("lead", SaidKind::Answer, "Because."), said("user", SaidKind::Reopened, "No."), said("lead", SaidKind::Answer, "Now.")]), &world(), &open);
    assert_eq!(again.iter().map(|c| match c { Change::Open(n) => format!("open {}", n.id), Change::End { id, .. } => format!("end {id}") }).collect::<Vec<_>>(), ["end disc:d3:2", "open disc:d3:4"]);
    // An agent asking something back: the user's move.
    let back = derive(&d(DiscussionStatus::Open, "user", "lead", vec![said("user", SaidKind::Message, "Why?"), said("lead", SaidKind::Message, "Which test?")]), &world(), &none);
    assert_eq!(opened(&back)[0].3, "lead said something back in d3 on plan p");
    let asked = derive(&d(DiscussionStatus::Open, "lead", "user", vec![said("lead", SaidKind::Message, "Is this list complete?")]), &world(), &none);
    assert_eq!(opened(&asked)[0].3, "lead asks you on plan p (d3)", "an agent's question to the user: {asked:?}");
    assert_eq!(derive(&ClientEvent::DiscussionRemoved { discussion_id: DiscussionId::new("d3") }, &world(), &open), [Change::End { id: NotificationId::new("disc:d3:2"), to: NotificationState::Done }]);
}
