//! What every shipped harness pack must do, checked against a mock of its
//! agent set up from the pack's own settings: its settings are the ACP
//! driver's; each of our modes it offers is one of its modes and back;
//! permission requests are asked, answered and skipped as the mode says;
//! messages sent while a turn runs all get answered, in order; what it
//! declares (steering, questions, who it's signed in as, logins) works;
//! its skill and tool-server places are all filled in.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use harness_acp::testing::{MockAgent, MockSetup};
use harness_acp::{AcpAdapter, AcpLauncher, AcpSettings, Profile, probe};
use harness_contracts::*;
use harness_core::{ProviderAdapter, SessionRuntime};
use meta_harness::packs::distribution;
use nah_packs::{Catalog, Kind, Places};

/// Every shipped harness pack: its name and its settings.
fn shipped() -> Vec<(String, AcpSettings)> {
    let dir = tempfile::tempdir().unwrap();
    let places = Places { project: dir.path().join("p"), home: dir.path().join("h"), config_home: dir.path().join("c"), data_home: dir.path().join("d") };
    let catalog = Catalog::load(&distribution(), &places).unwrap();
    let packs: Vec<(String, AcpSettings)> = catalog
        .of_kind(Kind::Harness)
        .map(|pack| {
            let provides = pack.manifest.provides.as_ref().unwrap_or_else(|| panic!("{} provides a harness", pack.name()));
            assert_eq!(provides.driver, harness_acp::DRIVER_KIND, "{} runs on the ACP driver", pack.name());
            let settings = AcpSettings::parse(&serde_json::to_value(&provides.settings).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", pack.name()));
            (pack.name().to_owned(), settings)
        })
        .collect();
    assert!(packs.len() >= 2, "Antigravity and Claude Code ship");
    packs
}

const PROJECT: &str = "/work/project";

struct Harness {
    name: String,
    profile: Arc<Profile>,
    agent: MockAgent,
}

impl Harness {
    fn all() -> Vec<Self> {
        shipped()
            .into_iter()
            .map(|(name, settings)| {
                let agent = MockAgent::new(MockSetup::like(&settings));
                Self { name, profile: Arc::new(Profile::new(settings, Path::new(PROJECT), None)), agent }
            })
            .collect()
    }

    async fn open(&self, mode: RuntimeMode) -> Box<dyn SessionRuntime> {
        let config = SessionConfig { thread_id: ThreadId::new("t"), policy: RuntimePolicy { runtime_mode: mode, ..RuntimePolicy::default() }, model: None, resume: None, mcp_servers: Vec::new() };
        AcpAdapter::new(InstanceId::new(&self.name), self.profile.clone(), Arc::new(self.agent.clone()), Arc::new(|_| {})).open_session(config).await.unwrap_or_else(|e| panic!("{}: {e}", self.name))
    }
}

fn say(text: &str) -> TurnInput {
    TurnInput { text: text.into(), attachments: Vec::new() }
}

async fn until(events: &mut (impl futures::Stream<Item = AdapterEvent> + Unpin), what: &str, done: impl Fn(&AdapterEvent) -> bool) -> AdapterEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.next().await {
            if done(&event) {
                return event;
            }
        }
        panic!("events ended before {what}");
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

fn ended(ordinal: u32) -> impl Fn(&AdapterEvent) -> bool {
    move |e| matches!(e, AdapterEvent::TurnUpdated { turn } if turn.ordinal == ordinal && turn.status.is_terminal())
}

#[tokio::test]
async fn each_mode_it_offers_is_one_of_its_own_and_back() {
    for h in Harness::all() {
        let offered = h.profile.runtime_modes();
        assert!(offered.contains(&RuntimeMode::ApprovalRequired), "{}: asking first is offered", h.name);
        for mode in offered {
            let session = h.open(mode).await;
            let native = h.agent.seen().modes.last().cloned().or_else(|| MockSetup::like(&h.profile.settings).modes.first().cloned()).unwrap();
            assert_eq!(h.profile.policy_for(&native, RuntimePolicy::default()).runtime_mode, mode, "{}: {mode:?} is {native} and back", h.name);
            session.close().await.unwrap();
        }
    }
}

#[tokio::test]
async fn permission_requests_are_asked_answered_and_skipped_as_the_mode_says() {
    for h in Harness::all() {
        let session = h.open(RuntimeMode::ApprovalRequired).await;
        let mut events = session.events();
        session.start_turn(say("[permission]")).await.unwrap();
        let AdapterEvent::RequestUpdated { request } = until(&mut events, "a permission request", |e| matches!(e, AdapterEvent::RequestUpdated { request } if request.status == RequestStatus::Pending)).await else { unreachable!() };
        let reject = request.options.iter().find(|o| o.decision == ApprovalDecision::Decline).unwrap_or_else(|| panic!("{}: a way to say no", h.name));
        session.respond(&request.id, RequestResponse { option_id: Some(reject.id.clone()), ..Default::default() }).await.unwrap();
        until(&mut events, "the turn's end", ended(1)).await;
        assert_eq!(h.agent.seen().permission.as_deref(), Some("reject"), "{}", h.name);
        if h.profile.runtime_modes().contains(&RuntimeMode::FullAccess) {
            let session = h.open(RuntimeMode::FullAccess).await;
            let mut events = session.events();
            session.start_turn(say("[permission]")).await.unwrap();
            until(&mut events, "the turn's end", ended(1)).await;
            assert_eq!(h.agent.seen().permission.as_deref(), Some("allow-once"), "{}: full access asks nothing", h.name);
        }
    }
}

/// A question asked through a permission request (as Antigravity asks
/// them) is a question in every harness, and no mode answers it for the user.
#[tokio::test]
async fn a_question_asked_through_a_permission_request_is_never_answered_for_the_user() {
    for h in Harness::all() {
        let mode = h.profile.runtime_modes().into_iter().find(|m| *m == RuntimeMode::FullAccess).unwrap_or(RuntimeMode::ApprovalRequired);
        let session = h.open(mode).await;
        let mut events = session.events();
        session.start_turn(say("[ask-by-permission]")).await.unwrap();
        let AdapterEvent::RequestUpdated { request } = until(&mut events, "the question", |e| matches!(e, AdapterEvent::RequestUpdated { request } if request.status == RequestStatus::Pending)).await else { unreachable!() };
        assert_eq!((request.kind, h.agent.seen().permission), (RequestKind::Question, None), "{}: asked, not answered", h.name);
        assert!(request.options.iter().all(|o| o.decision == ApprovalDecision::Answer), "{}: its options are answers", h.name);
    }
}

#[tokio::test]
async fn messages_sent_while_a_turn_runs_are_all_answered_in_order() {
    for h in Harness::all() {
        let session = h.open(RuntimeMode::ApprovalRequired).await;
        let mut events = session.events();
        session.start_turn(say("[hold] one")).await.unwrap();
        session.start_turn(say("two")).await.unwrap();
        session.start_turn(say("three")).await.unwrap();
        h.agent.prompted(1).await;
        h.agent.release();
        until(&mut events, "the third turn's end", ended(3)).await;
        assert_eq!(h.agent.seen().prompts, ["[hold] one", "two", "three"], "{}", h.name);
    }
}

#[tokio::test]
async fn what_it_declares_works() {
    for h in Harness::all() {
        let settings = &h.profile.settings;
        // Steering, if it takes it.
        if settings.steering.is_some() {
            let session = h.open(RuntimeMode::ApprovalRequired).await;
            let mut events = session.events();
            let turn = session.start_turn(say("[hold] one")).await.unwrap();
            h.agent.prompted(1).await;
            // Taken: it starts the next turn, which the prompt goes on in.
            assert_ne!(session.inject(say("also")).await.unwrap(), turn, "{}", h.name);
            h.agent.release();
            until(&mut events, "the turn's end", ended(2)).await;
            assert_eq!(h.agent.seen().steered, ["also"], "{}", h.name);
        }
        // Questions, if it asks them in forms.
        if settings.advertise.elicitation_form {
            let session = h.open(RuntimeMode::ApprovalRequired).await;
            let mut events = session.events();
            session.start_turn(say("[question]")).await.unwrap();
            let AdapterEvent::RequestUpdated { request } = until(&mut events, "questions", |e| matches!(e, AdapterEvent::RequestUpdated { request } if request.kind == RequestKind::Question)).await else { unreachable!() };
            assert_eq!(request.questions.len(), 2, "{}: each answer box folds into its question", h.name);
            let answers = [("question_0".to_owned(), vec!["Redis".to_owned()])].into();
            session.respond(&request.id, RequestResponse { option_id: Some("submit".into()), answers }).await.unwrap();
            until(&mut events, "the turn's end", ended(1)).await;
            let answer = serde_json::to_value(h.agent.seen().answers.unwrap()).unwrap();
            assert_eq!(answer, serde_json::json!({ "action": "accept", "content": { "question_0": "Redis" } }), "{}", h.name);
        }
        // Who it's signed in as, and its logins.
        let launcher: Arc<dyn AcpLauncher> = Arc::new(h.agent.clone());
        let found = probe(&launcher, &h.profile).await.unwrap();
        if settings.sign_in.is_some() {
            let sign_in = found.sign_in.unwrap_or_else(|| panic!("{}: says who it's signed in as", h.name));
            assert_eq!((sign_in.status, sign_in.account.map(|a| a.label)), (AuthStatus::SignedIn, Some("Test account".into())), "{}", h.name);
        }
        let kind = if settings.advertise.auth_terminal { AuthMethodKind::Command } else { AuthMethodKind::Agent };
        assert!(found.auth_methods.iter().all(|m| m.kind == kind), "{}: logins are {kind:?}", h.name);
        assert_eq!(found.logins.is_empty(), kind == AuthMethodKind::Agent, "{}", h.name);
    }
}

#[test]
fn its_skill_and_tool_server_places_are_all_filled_in() {
    for (name, settings) in shipped() {
        let profile = Profile::new(settings, Path::new(PROJECT), None);
        let skills = profile.settings.skills.as_ref().map(|s| profile.places.skill_roots(s)).unwrap_or_default();
        assert!(!skills.is_empty(), "{name}: says where its skills are");
        for root in skills {
            assert!(!root.path.display().to_string().contains('{'), "{name}: {}", root.path.display());
        }
        for file in profile.places.tool_files(&profile.settings.tool_config) {
            assert!(!file.path.display().to_string().contains('{') && !file.pointer.contains('{'), "{name}: {file:?}");
        }
    }
}
