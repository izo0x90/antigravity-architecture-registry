use meta_harness::protocol::*;
use meta_harness::testing::{AdapterTestHarness, HarnessOptions};

const THREAD_ID: &str = "antigravity-steering-thread";
const NATIVE_DEFAULT: &str = "gemini-test-low";
const NATIVE_ALTERNATIVE: &str = "gemini-test-high";
const NATIVE_SESSION_ID: &str = "b75db7e9-cd99-40e5-aa63-ac2b4674a6a9";

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 405-440:
/// "reapplies the exact saved model and mode after a native resume"
#[tokio::test]
async fn test_reapplies_the_exact_saved_model_and_mode_after_a_native_resume() {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir().unwrap().to_str().unwrap().to_string();

    let first = h
        .adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd: cwd.clone(),
            runtime_mode: "auto-accept-edits".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: Some(NATIVE_ALTERNATIVE.to_string()),
        })
        .await
        .expect("start first session");
    assert_eq!(first.model, NATIVE_ALTERNATIVE);

    h.adapter.stop_session(THREAD_ID).await.expect("stop session");

    let second = h
        .adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd: "/tmp".to_string(),
            runtime_mode: "auto-accept-edits".to_string(),
            resume_cursor: Some(first.resume_cursor),
            resume_session_id: None,
            model: Some(NATIVE_ALTERNATIVE.to_string()),
        })
        .await
        .expect("start second session");
    assert_eq!(second.model, NATIVE_ALTERNATIVE);
    assert_eq!(second.cwd, "/tmp");

    let launches = h.launches.lock().await;
    assert_eq!(
        launches[1].resume_session_id.as_deref(),
        Some(NATIVE_SESSION_ID)
    );

    assert_eq!(
        *h.calls.lock().await,
        vec![
            "start",
            &format!("model:{}", NATIVE_ALTERNATIVE),
            "mode:auto_edit",
            "start",
            &format!("model:{}", NATIVE_ALTERNATIVE),
            "mode:auto_edit",
        ]
    );

    assert_eq!(
        h.command_updates.lock().await.last().unwrap(),
        &vec!["plan".to_string(), "logout".to_string()]
    );

    assert!(!h.adapter.capabilities().supports_conversation_rollback);
    let rollback = h.adapter.rollback_thread(THREAD_ID, 1).await;
    assert!(rollback.is_err());
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 592-650:
/// "waits for native cancellation before a steer changes the model"
#[tokio::test]
async fn test_waits_for_native_cancellation_before_a_steer_changes_the_model() {
    let h = AdapterTestHarness::new_with_options(HarnessOptions {
        hold_cancel: true,
        ..Default::default()
    })
    .await;

    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd: std::env::current_dir().unwrap().to_str().unwrap().to_string(),
            runtime_mode: "approval-required".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start_session");

    let sending_first = {
        let adapter = h.adapter.clone();
        tokio::spawn(async move {
            adapter
                .send_turn(SendTurnInput {
                    thread_id: THREAD_ID.to_string(),
                    input: "First prompt".to_string(),
                    model: None,
                })
                .await
                .expect("send_turn first")
        })
    };

    let initial_prompt = h.next_prompt().await;
    assert!(initial_prompt.content.contains("First prompt"));
    assert!(initial_prompt
        .content
        .contains(&format!("Antigravity harness, as {}", NATIVE_DEFAULT)));

    let marker = h.calls.lock().await.len();

    let sending_second = {
        let adapter = h.adapter.clone();
        tokio::spawn(async move {
            adapter
                .send_turn(SendTurnInput {
                    thread_id: THREAD_ID.to_string(),
                    input: "Steer the turn".to_string(),
                    model: Some(NATIVE_ALTERNATIVE.to_string()),
                })
                .await
                .expect("send_turn second")
        })
    };

    let cancel_idx = h.next_cancellation().await;
    assert_eq!(cancel_idx, 1);
    assert_eq!(h.calls.lock().await[marker..], ["cancel:1"]);

    h.emit_native(NativeEvent::ContentDelta {
        text: "The first prompt stopped.".to_string(),
    })
    .await;

    h.release_cancel();

    let replacement = h.next_prompt().await;
    assert!(replacement.content.contains("Steer the turn"));
    assert!(replacement
        .content
        .contains(&format!("Antigravity harness, as {}", NATIVE_ALTERNATIVE)));

    assert_eq!(
        h.calls.lock().await[marker..],
        [
            "cancel:1",
            "drained:1",
            &format!("model:{}", NATIVE_ALTERNATIVE),
            "mode:default",
            "prompt:2",
        ]
    );

    replacement
        .resolve(PromptResponse {
            stop_reason: "end_turn".to_string(),
        })
        .await;

    let old_res = sending_first.await.expect("join sending_first");
    let new_res = sending_second.await.expect("join sending_second");
    assert_eq!(old_res.turn_id, new_res.turn_id);

    h.wait_for_event(|event| matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }))
        .await;

    let seen = h.seen.lock().await;
    let completed_count = seen
        .iter()
        .filter(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. }))
        .count();
    assert_eq!(completed_count, 1);

    let sessions = h.adapter.list_sessions().await;
    assert_eq!(sessions[0].status, "ready");
    assert_eq!(sessions[0].active_turn_id, None);
    assert_eq!(sessions[0].model, NATIVE_ALTERNATIVE);
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 652-677:
/// "rejects an unavailable steer model without cancelling current work"
#[tokio::test]
async fn test_rejects_an_unavailable_steer_model_without_cancelling_current_work() {
    let h = AdapterTestHarness::new().await;

    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd: std::env::current_dir().unwrap().to_str().unwrap().to_string(),
            runtime_mode: "approval-required".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start_session");

    let sending_first = {
        let adapter = h.adapter.clone();
        tokio::spawn(async move {
            adapter
                .send_turn(SendTurnInput {
                    thread_id: THREAD_ID.to_string(),
                    input: "Keep working".to_string(),
                    model: None,
                })
                .await
                .expect("send_turn first")
        })
    };

    let prompt = h.next_prompt().await;

    let invalid = h
        .adapter
        .send_turn(SendTurnInput {
            thread_id: THREAD_ID.to_string(),
            input: "Change model".to_string(),
            model: Some("not-in-this-account".to_string()),
        })
        .await;

    assert!(invalid.is_err());
    assert!(!h
        .calls
        .lock()
        .await
        .iter()
        .any(|call| call.starts_with("cancel:")));
    assert!(h.has_active_prompt().await);

    prompt
        .resolve(PromptResponse {
            stop_reason: "end_turn".to_string(),
        })
        .await;

    let _ = sending_first.await.expect("join sending_first");
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 679-714:
/// "settles a failed steer configuration and allows a later turn"
#[tokio::test]
async fn test_settles_a_failed_steer_configuration_and_allows_a_later_turn() {
    let h = AdapterTestHarness::new().await;

    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd: std::env::current_dir().unwrap().to_str().unwrap().to_string(),
            runtime_mode: "approval-required".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start_session");

    let sending_first = {
        let adapter = h.adapter.clone();
        tokio::spawn(async move {
            adapter
                .send_turn(SendTurnInput {
                    thread_id: THREAD_ID.to_string(),
                    input: "First".to_string(),
                    model: None,
                })
                .await
        })
    };

    let _prompt = h.next_prompt().await;
    h.controls.lock().await.fail_model = true;

    let failed = h
        .adapter
        .send_turn(SendTurnInput {
            thread_id: THREAD_ID.to_string(),
            input: "Replacement".to_string(),
            model: Some(NATIVE_ALTERNATIVE.to_string()),
        })
        .await;

    assert!(failed.is_err());

    let _ = sending_first.await;

    let ended = h
        .wait_for_event(|event| matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }))
        .await;

    let ended_turn_id = match ended {
        ProviderRuntimeEvent::TurnCompleted { turn_id, payload } => {
            assert_eq!(payload.state, TurnState::Failed);
            turn_id
        }
        _ => unreachable!(),
    };

    let sessions = h.adapter.list_sessions().await;
    assert_eq!(sessions[0].status, "error");
    assert_eq!(sessions[0].active_turn_id, None);

    let sending_later = {
        let adapter = h.adapter.clone();
        tokio::spawn(async move {
            adapter
                .send_turn(SendTurnInput {
                    thread_id: THREAD_ID.to_string(),
                    input: "Try again".to_string(),
                    model: None,
                })
                .await
                .expect("send_turn later")
        })
    };

    let later_prompt = h.next_prompt().await;
    later_prompt
        .resolve(PromptResponse {
            stop_reason: "end_turn".to_string(),
        })
        .await;

    let recovered = sending_later.await.expect("join sending_later");
    assert_ne!(recovered.turn_id, ended_turn_id);

    let sessions = h.adapter.list_sessions().await;
    assert_eq!(sessions[0].status, "ready");
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 716-737:
/// "cancels the native prompt if its send caller is interrupted"
#[tokio::test]
async fn test_cancels_the_native_prompt_if_its_send_caller_is_interrupted() {
    let h = AdapterTestHarness::new().await;

    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd: std::env::current_dir().unwrap().to_str().unwrap().to_string(),
            runtime_mode: "approval-required".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start_session");

    let sending = {
        let adapter = h.adapter.clone();
        tokio::spawn(async move {
            adapter
                .send_turn(SendTurnInput {
                    thread_id: THREAD_ID.to_string(),
                    input: "Keep working".to_string(),
                    model: None,
                })
                .await
        })
    };

    let _prompt = h.next_prompt().await;
    sending.abort();

    let ended = h
        .wait_for_event(|event| matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }))
        .await;

    match ended {
        ProviderRuntimeEvent::TurnCompleted { payload, .. } => {
            assert_eq!(payload.state, TurnState::Cancelled);
        }
        _ => unreachable!(),
    }

    assert!(!h.has_active_prompt().await);

    let sessions = h.adapter.list_sessions().await;
    assert_eq!(sessions[0].status, "ready");
    assert_eq!(sessions[0].active_turn_id, None);
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 739-776:
/// "tracks native commands that survive a turn and clears terminal tasks"
#[tokio::test]
async fn test_tracks_native_commands_that_survive_a_turn_and_clears_terminal_tasks() {
    let h = AdapterTestHarness::new().await;

    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd: std::env::current_dir().unwrap().to_str().unwrap().to_string(),
            runtime_mode: "approval-required".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start_session");

    let sending = {
        let adapter = h.adapter.clone();
        tokio::spawn(async move {
            adapter
                .send_turn(SendTurnInput {
                    thread_id: THREAD_ID.to_string(),
                    input: "Start a watcher".to_string(),
                    model: None,
                })
                .await
                .expect("send_turn")
        })
    };

    let prompt = h.next_prompt().await;

    h.emit_native(NativeEvent::ToolCallUpdated {
        tool_call: ToolCallState {
            tool_call_id: "watcher-1".to_string(),
            kind: "execute".to_string(),
            status: "inProgress".to_string(),
            command: Some("watch files".to_string()),
            ..Default::default()
        },
    })
    .await;

    prompt
        .resolve(PromptResponse {
            stop_reason: "end_turn".to_string(),
        })
        .await;

    let turn = sending.await.expect("join sending");

    let started = h
        .wait_for_event(|event| matches!(event, ProviderRuntimeEvent::TaskStarted { .. }))
        .await;

    let started_task_id = match started {
        ProviderRuntimeEvent::TaskStarted { turn_id, payload } => {
            assert_eq!(payload.task_type, "local_bash");
            assert_eq!(turn_id, turn.turn_id);
            payload.task_id
        }
        _ => unreachable!(),
    };

    h.emit_native(NativeEvent::ToolCallUpdated {
        tool_call: ToolCallState {
            tool_call_id: "watcher-1".to_string(),
            kind: "execute".to_string(),
            status: "completed".to_string(),
            ..Default::default()
        },
    })
    .await;

    let ended = h
        .wait_for_event(|event| matches!(event, ProviderRuntimeEvent::TaskCompleted { .. }))
        .await;

    match ended {
        ProviderRuntimeEvent::TaskCompleted { payload, .. } => {
            assert_eq!(payload.task_id, started_task_id);
            assert_eq!(payload.status, "completed");
        }
        _ => unreachable!(),
    }
}
