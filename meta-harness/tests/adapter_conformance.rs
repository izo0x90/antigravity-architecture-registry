use std::collections::HashMap;

use meta_harness::protocol::*;
use meta_harness::testing::AdapterTestHarness;

const THREAD_ID: &str = "thread-1";
const NATIVE_SESSION_ID: &str = "b75db7e9-cd99-40e5-aa63-ac2b4674a6a9";

/// Ported from AntigravityAdapter.test.ts lines 442-488:
/// "keeps thoughts, native command results, and replies on the active turn"
#[tokio::test]
async fn test_keeps_thoughts_native_command_results_and_replies_on_active_turn() {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir()
        .expect("cwd")
        .to_string_lossy()
        .to_string();

    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd,
            runtime_mode: "approval-required".to_string(),
            ..Default::default()
        })
        .await
        .expect("start_session");

    let sending = tokio::spawn({
        let adapter = h.adapter.clone();
        async move {
            adapter
                .send_turn(SendTurnInput {
                    thread_id: THREAD_ID.to_string(),
                    input: "Read the file".to_string(),
                    model: None,
                })
                .await
                .expect("send_turn")
        }
    });

    let prompt = h.next_prompt().await;

    h.emit_native(NativeEvent::ThoughtDelta {
        text: "I will read it.".to_string(),
    })
    .await;

    h.emit_native(NativeEvent::ToolCallUpdated {
        tool_call: ToolCallState {
            tool_call_id: "command-1".to_string(),
            kind: "execute".to_string(),
            status: "completed".to_string(),
            data: Some(serde_json::json!({
                "rawInput": { "CommandLine": "cat probe.txt", "Cwd": "/tmp" },
                "rawOutput": { "combinedOutput": "after\n", "exitCode": 0 },
            })),
            ..Default::default()
        },
    })
    .await;

    h.emit_native(NativeEvent::ContentDelta {
        text: "The file says after.".to_string(),
    })
    .await;

    prompt
        .resolve(PromptResponse {
            stop_reason: "end_turn".to_string(),
        })
        .await;

    let result = sending.await.expect("join sending");

    h.wait_for_event(|event| matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }))
        .await;

    let seen = h.seen.lock().await;
    let deltas: Vec<&ProviderRuntimeEvent> = seen
        .iter()
        .filter(|event| matches!(event, ProviderRuntimeEvent::ContentDelta { .. }))
        .collect();

    let stream_kinds: Vec<StreamKind> = deltas
        .iter()
        .map(|event| match event {
            ProviderRuntimeEvent::ContentDelta { payload, .. } => payload.stream_kind,
            _ => unreachable!(),
        })
        .collect();

    assert_eq!(
        stream_kinds,
        vec![StreamKind::ReasoningText, StreamKind::AssistantText]
    );

    assert!(deltas.iter().all(|event| match event {
        ProviderRuntimeEvent::ContentDelta { turn_id, .. } => turn_id == &result.turn_id,
        _ => false,
    }));

    let tool = seen.iter().find(|event| {
        matches!(
            event,
            ProviderRuntimeEvent::ItemCompleted { payload, .. }
                if payload.item_type == "command_execution"
        )
    });

    let tool_data = match tool {
        Some(ProviderRuntimeEvent::ItemCompleted { payload, .. }) => &payload.data,
        _ => panic!("Expected command_execution tool completed event"),
    };

    assert_eq!(
        tool_data.get("command").and_then(|v| v.as_str()),
        Some("cat probe.txt")
    );
    assert_eq!(
        tool_data.get("cwd").and_then(|v| v.as_str()),
        Some("/tmp")
    );
    assert_eq!(
        tool_data
            .get("item")
            .and_then(|i| i.get("aggregatedOutput"))
            .and_then(|v| v.as_str()),
        Some("after\n")
    );
    assert_eq!(
        tool_data
            .get("item")
            .and_then(|i| i.get("exitCode"))
            .and_then(|v| v.as_i64()),
        Some(0)
    );
}

/// Ported from AntigravityAdapter.test.ts lines 490-525:
/// "does not auto-approve a remaining native request in full access"
#[tokio::test]
async fn test_does_not_auto_approve_remaining_native_request_in_full_access() {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir()
        .expect("cwd")
        .to_string_lossy()
        .to_string();

    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd,
            runtime_mode: "full-access".to_string(),
            ..Default::default()
        })
        .await
        .expect("start_session");

    let mut permission = h
        .invoke_permission(PermissionRequest {
            session_id: NATIVE_SESSION_ID.to_string(),
            tool_call: ToolCallState {
                tool_call_id: "write-1".to_string(),
                kind: "edit".to_string(),
                title: Some("Write probe.txt".to_string()),
                ..Default::default()
            },
            options: vec![
                PermissionOption {
                    option_id: "native:allow".to_string(),
                    name: "Allow".to_string(),
                    kind: "allow_once".to_string(),
                },
                PermissionOption {
                    option_id: "native:deny".to_string(),
                    name: "Deny".to_string(),
                    kind: "reject_once".to_string(),
                },
            ],
        })
        .await;

    let opened = h
        .wait_for_event(|event| matches!(event, ProviderRuntimeEvent::RequestOpened { .. }))
        .await;

    let (request_id, options) = match opened {
        ProviderRuntimeEvent::RequestOpened {
            request_id,
            payload,
        } => (request_id, payload.options),
        _ => unreachable!(),
    };

    let calls = h.calls.lock().await;
    assert!(calls.contains(&"mode:yolo".to_string()));
    drop(calls);

    assert_eq!(
        options,
        vec![
            ApprovalOption {
                decision: "accept".to_string(),
                label: "Allow once".to_string()
            },
            ApprovalOption {
                decision: "decline".to_string(),
                label: "Deny".to_string()
            },
            ApprovalOption {
                decision: "cancel".to_string(),
                label: "Cancel".to_string()
            },
        ]
    );

    assert!(!permission.is_resolved());

    let always = h
        .adapter
        .respond_to_request(THREAD_ID, &request_id, "acceptAlways")
        .await;
    assert!(always.is_err());

    let decline = h
        .adapter
        .respond_to_request(THREAD_ID, &request_id, "decline")
        .await;
    assert!(decline.is_ok());

    let outcome = permission.await_outcome().await.expect("join permission");
    assert_eq!(
        outcome,
        PermissionOutcome::Selected {
            option_id: "native:deny".to_string()
        }
    );
}

/// Ported from AntigravityAdapter.test.ts lines 527-561:
/// "returns opaque native question choices and rejects ambiguous labels"
#[tokio::test]
async fn test_returns_opaque_native_question_choices_and_rejects_ambiguous_labels() {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir()
        .expect("cwd")
        .to_string_lossy()
        .to_string();

    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd,
            runtime_mode: "full-access".to_string(),
            ..Default::default()
        })
        .await
        .expect("start_session");

    let mut question = h
        .invoke_permission(PermissionRequest {
            session_id: NATIVE_SESSION_ID.to_string(),
            tool_call: ToolCallState {
                tool_call_id: "interaction_opaque".to_string(),
                kind: "interaction".to_string(),
                title: Some("Which target?".to_string()),
                ..Default::default()
            },
            options: vec![
                PermissionOption {
                    option_id: "choice:a".to_string(),
                    name: "Same label".to_string(),
                    kind: "allow_once".to_string(),
                },
                PermissionOption {
                    option_id: "choice:b".to_string(),
                    name: "Same label".to_string(),
                    kind: "allow_once".to_string(),
                },
            ],
        })
        .await;

    let opened = h
        .wait_for_event(|event| matches!(event, ProviderRuntimeEvent::UserInputRequested { .. }))
        .await;

    let (request_id, questions) = match opened {
        ProviderRuntimeEvent::UserInputRequested {
            request_id,
            payload,
        } => (request_id, payload.questions),
        _ => unreachable!(),
    };

    assert!(!questions[0].allow_custom_answer);
    assert_eq!(
        questions[0]
            .options
            .iter()
            .map(|opt| opt.value.clone())
            .collect::<Vec<_>>(),
        vec!["choice:a".to_string(), "choice:b".to_string()]
    );

    let mut invalid_answers = HashMap::new();
    invalid_answers.insert(
        "interaction_opaque".to_string(),
        "Same label".to_string(),
    );
    let invalid = h
        .adapter
        .respond_to_user_input(THREAD_ID, &request_id, &invalid_answers)
        .await;
    assert!(invalid.is_err());

    assert!(!question.is_resolved());

    let mut valid_answers = HashMap::new();
    valid_answers.insert(
        "interaction_opaque".to_string(),
        "choice:b".to_string(),
    );
    let valid = h
        .adapter
        .respond_to_user_input(THREAD_ID, &request_id, &valid_answers)
        .await;
    assert!(valid.is_ok());

    let outcome = question.await_outcome().await.expect("join question");
    assert_eq!(
        outcome,
        PermissionOutcome::Selected {
            option_id: "choice:b".to_string()
        }
    );
}

/// Ported from AntigravityAdapter.test.ts lines 563-590:
/// "cancels native questions before waiting for the prompt to end"
#[tokio::test]
async fn test_cancels_native_questions_before_waiting_for_the_prompt_to_end() {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir()
        .expect("cwd")
        .to_string_lossy()
        .to_string();

    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd,
            runtime_mode: "approval-required".to_string(),
            ..Default::default()
        })
        .await
        .expect("start_session");

    let sending = tokio::spawn({
        let adapter = h.adapter.clone();
        async move {
            adapter
                .send_turn(SendTurnInput {
                    thread_id: THREAD_ID.to_string(),
                    input: "Ask a question".to_string(),
                    model: None,
                })
                .await
                .expect("send_turn")
        }
    });

    let _prompt = h.next_prompt().await;

    let question = h
        .invoke_permission(PermissionRequest {
            session_id: NATIVE_SESSION_ID.to_string(),
            tool_call: ToolCallState {
                tool_call_id: "interaction_cancel".to_string(),
                kind: "interaction".to_string(),
                title: Some("Continue?".to_string()),
                ..Default::default()
            },
            options: vec![PermissionOption {
                option_id: "yes".to_string(),
                name: "Yes".to_string(),
                kind: "allow_once".to_string(),
            }],
        })
        .await;

    h.wait_for_event(|event| matches!(event, ProviderRuntimeEvent::UserInputRequested { .. }))
        .await;

    h.adapter
        .interrupt_turn(THREAD_ID)
        .await
        .expect("interrupt_turn");

    let question_outcome = question.await_outcome().await.expect("join question");
    assert_eq!(question_outcome, PermissionOutcome::Cancelled);

    let _ = sending.await.expect("join sending");

    let ended = h
        .wait_for_event(|event| matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }))
        .await;

    match ended {
        ProviderRuntimeEvent::TurnCompleted { payload, .. } => {
            assert_eq!(payload.state, TurnState::Cancelled);
        }
        _ => unreachable!(),
    }

    let seen = h.seen.lock().await;
    assert!(seen
        .iter()
        .any(|event| matches!(event, ProviderRuntimeEvent::UserInputResolved { .. })));
}
