use harness_contracts::*;
use serde_json::json;

#[test]
fn events_are_tagged_snake_case() {
    let event = AdapterEvent::UsageUpdated {
        thread_id: ThreadId::new("t1"),
        turn_id: None,
        usage: Usage { input_tokens: Some(10), ..Usage::default() },
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["type"], "usage_updated");
    assert_eq!(value["threadId"], "t1");
    assert_eq!(serde_json::from_value::<AdapterEvent>(value).unwrap(), event);
}

#[test]
fn unknown_item_body_decodes_as_unsupported() {
    let body: ItemBody = serde_json::from_value(json!({ "type": "hologram", "x": 1 })).unwrap();
    assert_eq!(body, ItemBody::Unsupported);
}

#[test]
fn unknown_content_block_decodes_as_unsupported() {
    let block: ContentBlock = serde_json::from_value(json!({ "type": "audio", "data": "…" })).unwrap();
    assert_eq!(block, ContentBlock::Unsupported);
}

#[test]
fn approval_options_round_trip() {
    let option = ApprovalOption {
        id: "allow_always".into(),
        decision: ApprovalDecision::AcceptForSession,
        label: "Allow for this thread".into(),
        warning: Some("Applies to every later call".into()),
    };
    let value = serde_json::to_value(&option).unwrap();
    assert_eq!(value["decision"], "accept_for_session");
    assert_eq!(serde_json::from_value::<ApprovalOption>(value).unwrap(), option);
}
