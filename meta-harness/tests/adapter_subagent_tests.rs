use meta_harness::protocol::*;
use meta_harness::testing::AdapterTestHarness;

const THREAD_ID: &str = "antigravity-subagent-thread";
const NATIVE_SESSION_ID: &str = "b75db7e9-cd99-40e5-aa63-ac2b4674a6a9";

#[derive(Default, Clone, Copy)]
struct McpFlags {
    is_mcp: bool,
    meta_is_mcp: bool,
}

const NO_MCP: McpFlags = McpFlags {
    is_mcp: false,
    meta_is_mcp: false,
};
const META_MCP: McpFlags = McpFlags {
    is_mcp: false,
    meta_is_mcp: true,
};

fn native_tool_update(
    id: &str,
    session_update: &str,
    status: &str,
    title: Option<&str>,
    kind: &str,
    output: Option<&str>,
    mcp: McpFlags,
) -> NativeEvent {
    let mut data = serde_json::Map::new();
    if mcp.meta_is_mcp {
        let mut meta = serde_json::Map::new();
        meta.insert("is_mcp_tool_call".to_string(), serde_json::Value::Bool(true));
        data.insert("_meta".to_string(), serde_json::Value::Object(meta));
    }
    NativeEvent::ToolCallUpdated {
        tool_call: ToolCallState {
            tool_call_id: id.to_string(),
            kind: kind.to_string(),
            status: status.to_string(),
            title: title.map(|t| t.to_string()),
            data: if data.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(data))
            },
            is_mcp: mcp.is_mcp,
            raw_output: output.map(|o| o.to_string()),
            command: None,
            session_update: Some(session_update.to_string()),
        },
    }
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 778-843:
/// "keeps a launched batch active while child tools continue"
#[tokio::test]
async fn test_keeps_a_launched_batch_active_while_child_tools_continue() {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir().unwrap().to_str().unwrap().to_string();
    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd,
            runtime_mode: "full-access".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start session");

    let adapter = h.adapter.clone();
    let sending = tokio::spawn(async move {
        adapter
            .send_turn(SendTurnInput {
                thread_id: THREAD_ID.to_string(),
                input: "Run two readers in one batch".to_string(),
                model: None,
            })
            .await
    });

    let prompt = h.next_prompt().await;
    let tool_call_id = format!("{}:2", NATIVE_SESSION_ID);
    let started = native_tool_update(
        &tool_call_id,
        "tool_call",
        "in_progress",
        Some("Running start_subagent"),
        "other",
        None,
        NO_MCP,
    );
    h.emit_native(started).await;

    let launched_initial = h
        .wait_for_event(|e| matches!(e, ProviderRuntimeEvent::TaskProgress { .. }))
        .await;
    assert!(matches!(
        launched_initial,
        ProviderRuntimeEvent::TaskProgress { .. }
    ));

    let updated = native_tool_update(
        &tool_call_id,
        "tool_call_update",
        "completed",
        Some("Running start_subagent"),
        "other",
        Some("Launch subagents"),
        NO_MCP,
    );
    h.emit_native(updated).await;

    let launched = h
        .wait_for_event(|e| {
            if let ProviderRuntimeEvent::TaskProgress { payload, .. } = e {
                payload.description.as_deref() == Some("Launch subagents")
            } else {
                false
            }
        })
        .await;

    if let ProviderRuntimeEvent::TaskProgress { payload, .. } = launched {
        assert_eq!(payload.task_id, tool_call_id);
        assert_eq!(payload.title, "Antigravity subagent batch");
        assert_eq!(payload.task_type, "subagent_batch");
        assert_eq!(payload.description.as_deref(), Some("Launch subagents"));
        assert_eq!(payload.status, "running");
    } else {
        panic!("expected TaskProgress");
    }

    for child in ["alpha", "beta"] {
        h.emit_native(native_tool_update(
            &format!("{}:1", child),
            "tool_call",
            "completed",
            Some("Read file"),
            "read",
            Some("File contents"),
            NO_MCP,
        ))
        .await;
    }

    h.drain_events().await;
    {
        let seen = h.seen.lock().await;
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(e, ProviderRuntimeEvent::TaskCompleted { .. }))
                .count(),
            0
        );
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(e, ProviderRuntimeEvent::TaskUpdated { .. }))
                .count(),
            0
        );
    }

    prompt
        .resolve(PromptResponse {
            stop_reason: "end_turn".to_string(),
        })
        .await;

    let _ = sending.await.unwrap().expect("send turn succeeded");
    h.wait_for_event(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. }))
        .await;

    let seen = h.seen.lock().await;
    assert_eq!(
        seen.iter()
            .filter(|e| matches!(e, ProviderRuntimeEvent::TaskCompleted { .. }))
            .count(),
        0
    );
    let updated_event = seen
        .iter()
        .find(|e| matches!(e, ProviderRuntimeEvent::TaskUpdated { .. }))
        .expect("task.updated event exists");

    if let ProviderRuntimeEvent::TaskUpdated { payload, .. } = updated_event {
        assert_eq!(payload.task_id, tool_call_id);
        assert_eq!(payload.status, "idle");
        assert_eq!(
            payload.description.as_deref(),
            Some("Turn ended. Individual agent status is unavailable.")
        );
        assert_eq!(payload.timeline_bypass, Some(true));
    }
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 845-883:
/// "waits for a replayed subagent's final status and result"
#[tokio::test]
async fn test_waits_for_a_replayed_subagents_final_status_and_result() {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir().unwrap().to_str().unwrap().to_string();
    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd,
            runtime_mode: "approval-required".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start session");

    // ACP history announces a completed tool first, even when its result failed.
    h.emit_native(native_tool_update(
        "replayed:4",
        "tool_call",
        "completed",
        Some("Running start_subagent"),
        "other",
        None,
        NO_MCP,
    ))
    .await;

    h.emit_native(native_tool_update(
        "replayed:4",
        "tool_call_update",
        "failed",
        Some("Running start_subagent"),
        "other",
        Some("Review failed."),
        NO_MCP,
    ))
    .await;

    let completed = h
        .wait_for_event(|e| matches!(e, ProviderRuntimeEvent::TaskCompleted { .. }))
        .await;

    if let ProviderRuntimeEvent::TaskCompleted { payload, .. } = completed {
        assert_eq!(payload.task_id, "replayed:4");
        assert_eq!(payload.task_type, "subagent_batch");
        assert_eq!(payload.tool_use_id.as_deref(), Some("replayed:4"));
        assert_eq!(payload.title, "Antigravity subagent batch");
        assert_eq!(payload.status, "failed");
        assert_eq!(payload.summary.as_deref(), Some("Review failed."));
    } else {
        panic!("expected TaskCompleted");
    }

    let seen = h.seen.lock().await;
    let task_events: Vec<_> = seen
        .iter()
        .filter(|e| {
            matches!(
                e,
                ProviderRuntimeEvent::TaskStarted { .. }
                    | ProviderRuntimeEvent::TaskProgress { .. }
                    | ProviderRuntimeEvent::TaskUpdated { .. }
                    | ProviderRuntimeEvent::TaskCompleted { .. }
            )
        })
        .collect();
    assert_eq!(task_events.len(), 1);
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 885-937:
/// "keeps one-message launches active and ignores late updates after settlement"
#[tokio::test]
async fn test_keeps_one_message_launches_active_and_ignores_late_updates_after_settlement() {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir().unwrap().to_str().unwrap().to_string();
    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd,
            runtime_mode: "full-access".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start session");

    let adapter = h.adapter.clone();
    let first = tokio::spawn(async move {
        adapter
            .send_turn(SendTurnInput {
                thread_id: THREAD_ID.to_string(),
                input: "Start readers".to_string(),
                model: None,
            })
            .await
    });
    let first_prompt = h.next_prompt().await;

    h.emit_native(native_tool_update(
        "old:0",
        "tool_call",
        "completed",
        Some("Running start_subagent"),
        "other",
        Some("Launch readers"),
        NO_MCP,
    ))
    .await;

    h.emit_native(native_tool_update(
        "old:1",
        "tool_call",
        "completed",
        Some("Running start_subagent"),
        "other",
        None,
        NO_MCP,
    ))
    .await;

    h.drain_events().await;
    {
        let seen = h.seen.lock().await;
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(e, ProviderRuntimeEvent::TaskProgress { .. }))
                .count(),
            2
        );
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(e, ProviderRuntimeEvent::TaskCompleted { .. }))
                .count(),
            0
        );
    }

    first_prompt
        .resolve(PromptResponse {
            stop_reason: "end_turn".to_string(),
        })
        .await;
    let _ = first.await.unwrap().expect("first turn succeeded");
    h.wait_for_event(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. }))
        .await;

    let adapter2 = h.adapter.clone();
    let second = tokio::spawn(async move {
        adapter2
            .send_turn(SendTurnInput {
                thread_id: THREAD_ID.to_string(),
                input: "Next task".to_string(),
                model: None,
            })
            .await
    });
    let second_prompt = h.next_prompt().await;

    for id in ["old:0", "old:1"] {
        for status in ["in_progress", "completed", "failed"] {
            h.emit_native(native_tool_update(
                id,
                "tool_call_update",
                status,
                Some("Running start_subagent"),
                "other",
                Some("Late update"),
                NO_MCP,
            ))
            .await;
        }
    }

    h.drain_events().await;
    {
        let seen = h.seen.lock().await;
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(e, ProviderRuntimeEvent::TaskProgress { .. }))
                .count(),
            2
        );
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(e, ProviderRuntimeEvent::TaskUpdated { .. }))
                .count(),
            2
        );
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(e, ProviderRuntimeEvent::TaskCompleted { .. }))
                .count(),
            0
        );
    }

    second_prompt
        .resolve(PromptResponse {
            stop_reason: "end_turn".to_string(),
        })
        .await;
    let _ = second.await.unwrap().expect("second turn succeeded");
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 939-971:
/// "does not report a historical launch as running or completed work"
#[tokio::test]
async fn test_does_not_report_a_historical_launch_as_running_or_completed_work() {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir().unwrap().to_str().unwrap().to_string();
    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd,
            runtime_mode: "full-access".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start session");

    h.emit_native(native_tool_update(
        "history:2",
        "tool_call",
        "completed",
        Some("Running start_subagent"),
        "other",
        None,
        NO_MCP,
    ))
    .await;

    h.emit_native(native_tool_update(
        "history:2",
        "tool_call_update",
        "completed",
        Some("Running start_subagent"),
        "other",
        Some("Launch readers"),
        NO_MCP,
    ))
    .await;

    h.drain_events().await;
    let seen = h.seen.lock().await;
    let task_events: Vec<_> = seen
        .iter()
        .filter(|e| {
            matches!(
                e,
                ProviderRuntimeEvent::TaskStarted { .. }
                    | ProviderRuntimeEvent::TaskProgress { .. }
                    | ProviderRuntimeEvent::TaskUpdated { .. }
                    | ProviderRuntimeEvent::TaskCompleted { .. }
            )
        })
        .collect();

    assert_eq!(task_events.len(), 1);
    if let ProviderRuntimeEvent::TaskUpdated { payload, .. } = task_events[0] {
        assert_eq!(payload.status, "idle");
        assert_eq!(payload.timeline_bypass, Some(true));
    } else {
        panic!("expected TaskUpdated");
    }
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 973-1015:
/// "keeps MCP identity when later updates omit metadata"
#[tokio::test]
async fn test_keeps_mcp_identity_when_later_updates_omit_metadata() {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir().unwrap().to_str().unwrap().to_string();
    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd,
            runtime_mode: "approval-required".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start session");

    let adapter = h.adapter.clone();
    let sending = tokio::spawn(async move {
        adapter
            .send_turn(SendTurnInput {
                thread_id: THREAD_ID.to_string(),
                input: "Run an MCP tool".to_string(),
                model: None,
            })
            .await
    });
    let prompt = h.next_prompt().await;

    h.emit_native(native_tool_update(
        "mcp-4",
        "tool_call",
        "in_progress",
        Some("Running start_subagent"),
        "other",
        None,
        META_MCP,
    ))
    .await;

    for status in ["in_progress", "completed"] {
        h.emit_native(native_tool_update(
            "mcp-4",
            "tool_call_update",
            status,
            Some("Running start_subagent"),
            "other",
            Some("MCP output."),
            NO_MCP,
        ))
        .await;
    }

    prompt
        .resolve(PromptResponse {
            stop_reason: "end_turn".to_string(),
        })
        .await;
    let _ = sending.await.unwrap().expect("send turn succeeded");
    h.wait_for_event(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. }))
        .await;

    let seen = h.seen.lock().await;
    let task_events_count = seen
        .iter()
        .filter(|e| {
            matches!(
                e,
                ProviderRuntimeEvent::TaskStarted { .. }
                    | ProviderRuntimeEvent::TaskProgress { .. }
                    | ProviderRuntimeEvent::TaskUpdated { .. }
                    | ProviderRuntimeEvent::TaskCompleted { .. }
            )
        })
        .count();
    assert_eq!(task_events_count, 0);

    let item_updated_count = seen
        .iter()
        .filter(|e| matches!(e, ProviderRuntimeEvent::ItemUpdated { .. }))
        .count();
    assert_eq!(item_updated_count, 2);

    let item_completed_count = seen
        .iter()
        .filter(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted { .. }))
        .count();
    assert_eq!(item_completed_count, 1);
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 1017-1047:
/// "shows pending subagents and closes a denied invocation"
#[tokio::test]
async fn test_shows_pending_subagents_and_closes_a_denied_invocation() {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir().unwrap().to_str().unwrap().to_string();
    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd,
            runtime_mode: "approval-required".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start session");

    h.emit_native(native_tool_update(
        "permission-1",
        "tool_call",
        "pending",
        Some("Run start_subagent?"),
        "other",
        None,
        NO_MCP,
    ))
    .await;

    let pending = h
        .wait_for_event(|e| matches!(e, ProviderRuntimeEvent::TaskProgress { .. }))
        .await;
    if let ProviderRuntimeEvent::TaskProgress { payload, .. } = pending {
        assert_eq!(payload.status, "pending");
    } else {
        panic!("expected TaskProgress");
    }

    h.emit_native(native_tool_update(
        "permission-1",
        "tool_call_update",
        "failed",
        Some("Run start_subagent?"),
        "other",
        None,
        NO_MCP,
    ))
    .await;

    let completed = h
        .wait_for_event(|e| matches!(e, ProviderRuntimeEvent::TaskCompleted { .. }))
        .await;
    if let ProviderRuntimeEvent::TaskCompleted { payload, .. } = completed {
        assert_eq!(payload.status, "failed");
    } else {
        panic!("expected TaskCompleted");
    }
}

/// Ported with 100% fidelity from AntigravityAdapter.test.ts lines 1049-1116:
/// "settles open subagent calls on cancel / steer / disconnect / end_turn"
#[tokio::test]
async fn test_settles_open_subagent_calls_on_cancel() {
    run_settles_open_subagent_calls_test("cancel").await;
}

#[tokio::test]
async fn test_settles_open_subagent_calls_on_steer() {
    run_settles_open_subagent_calls_test("steer").await;
}

#[tokio::test]
async fn test_settles_open_subagent_calls_on_disconnect() {
    run_settles_open_subagent_calls_test("disconnect").await;
}

#[tokio::test]
async fn test_settles_open_subagent_calls_on_end_turn() {
    run_settles_open_subagent_calls_test("end_turn").await;
}

async fn run_settles_open_subagent_calls_test(stop: &str) {
    let h = AdapterTestHarness::new().await;
    let cwd = std::env::current_dir().unwrap().to_str().unwrap().to_string();
    h.adapter
        .start_session(StartSessionInput {
            thread_id: THREAD_ID.to_string(),
            cwd,
            runtime_mode: "approval-required".to_string(),
            resume_cursor: None,
            resume_session_id: None,
            model: None,
        })
        .await
        .expect("start session");

    let adapter = h.adapter.clone();
    let sending = tokio::spawn(async move {
        adapter
            .send_turn(SendTurnInput {
                thread_id: THREAD_ID.to_string(),
                input: "Start a subagent".to_string(),
                model: None,
            })
            .await
    });
    let prompt = h.next_prompt().await;

    h.emit_native(native_tool_update(
        "trajectory:4",
        "tool_call",
        "in_progress",
        Some("Running start_subagent"),
        "other",
        None,
        NO_MCP,
    ))
    .await;
    h.wait_for_event(|e| matches!(e, ProviderRuntimeEvent::TaskProgress { .. }))
        .await;

    h.emit_native(native_tool_update(
        "trajectory:4",
        "tool_call_update",
        "completed",
        Some("Running start_subagent"),
        "other",
        Some("Launch subagents"),
        NO_MCP,
    ))
    .await;
    h.wait_for_event(|e| matches!(e, ProviderRuntimeEvent::TaskProgress { .. }))
        .await;

    if stop == "disconnect" {
        h.emit_native(NativeEvent::ConnectionTerminated {
            detail: "Process exited.".to_string(),
        })
        .await;
    } else if stop == "cancel" {
        h.adapter
            .interrupt_turn(THREAD_ID)
            .await
            .expect("interrupt turn");
    } else if stop == "steer" {
        let adapter_steer = h.adapter.clone();
        let steering = tokio::spawn(async move {
            adapter_steer
                .send_turn(SendTurnInput {
                    thread_id: THREAD_ID.to_string(),
                    input: "Change direction".to_string(),
                    model: None,
                })
                .await
        });
        let replacement = h.next_prompt().await;
        replacement
            .resolve(PromptResponse {
                stop_reason: "end_turn".to_string(),
            })
            .await;
        let _ = steering.await.unwrap().expect("steering turn succeeded");
    } else {
        prompt
            .resolve(PromptResponse {
                stop_reason: "end_turn".to_string(),
            })
            .await;
    }

    let settled = h
        .wait_for_event(|e| matches!(e, ProviderRuntimeEvent::TaskUpdated { .. }))
        .await;

    if let ProviderRuntimeEvent::TaskUpdated { payload, .. } = settled {
        assert_eq!(payload.task_id, "trajectory:4");
        assert_eq!(payload.title, "Antigravity subagent batch");
        assert_eq!(payload.task_type, "subagent_batch");
        let expected_status = match stop {
            "disconnect" => "failed",
            "cancel" | "steer" => "cancelled",
            _ => "idle",
        };
        assert_eq!(payload.status, expected_status);
    } else {
        panic!("expected TaskUpdated");
    }

    if stop == "disconnect" {
        h.wait_for_event(|e| matches!(e, ProviderRuntimeEvent::SessionExited { .. }))
            .await;
    } else {
        let _ = sending.await.unwrap();
    }
}
