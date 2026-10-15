//! Calls from outside: reading the screen, running, clicking, keys.

use super::*;

#[test]
fn from_outside_the_screen_is_read_and_actions_run_clicked_and_keyed_as_the_user_would() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Graph);
    let read = from_outside(&ctx, &mut app, r#"{"op":"read"}"#).unwrap();
    assert_eq!(read["mode"], "Normal");
    assert!(read["columns"].as_array().unwrap().iter().any(|c| c["focused"] == true && c["view"].as_str().unwrap().starts_with("graph")), "{}", read["columns"]);
    assert!(read["texts"].as_array().unwrap().iter().any(|t| t["text"] == "step"), "what's drawn, with where");

    let actions = from_outside(&ctx, &mut app, r#"{"op":"actions"}"#).unwrap();
    let add = actions.as_array().unwrap().iter().find(|a| a["id"] == "column.add").expect("listed");
    assert_eq!((add["keys"].as_str(), add["needs"].as_str()), (Some("SPC l a"), Some("nothing")));

    let before = app.desk.columns.len();
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"column.add"}"#).unwrap();
    assert_eq!(app.desk.columns.len(), before + 1, "run as its key would");
    let err = from_outside(&ctx, &mut app, r#"{"op":"run","id":"no.such"}"#).unwrap_err();
    assert!(err.contains("no action"), "{err}");
    let err = from_outside(&ctx, &mut app, r#"{"op":"run","id":"cockpit.agents","value":"maybe"}"#).unwrap_err();
    assert!(err.contains("isn't one of"), "{err}");
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"cockpit.agents","value":"on"}"#).unwrap();
    assert!(app.desk.remote.agents && app.saved_ui().agents, "the user's say, kept");
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"cockpit.agents","value":"off"}"#).unwrap();
    from_outside(&ctx, &mut app, r#"{"op":"run","id":"slot.focus.1"}"#).unwrap();
    focus(&ctx, &mut app, BufferKind::Graph);

    from_outside(&ctx, &mut app, r#"{"op":"keys","keys":"space"}"#).unwrap();
    assert_eq!(app.input_mode, InputMode::Leader, "keys as typed");
    from_outside(&ctx, &mut app, r#"{"op":"keys","keys":"escape"}"#).unwrap();
    assert_eq!(app.input_mode, InputMode::Normal);

    let mut tree = crate::graph::tests::sample_tree();
    tree.cut = Some(arch_graph::tree::Cut { reason: arch_graph::tree::CutReason::Budget, starts_left_out: 0, shown: 2, asked: 3, hidden: 7 });
    app.desk.graph.set_tree(tree);
    let clicked = from_outside(&ctx, &mut app, r#"{"op":"click","text":"Show 2 levels"}"#).unwrap();
    assert_eq!(clicked["clicked"], "Show 2 levels");
    assert_eq!(app.desk.graph.nav.depth, Some(2), "clicked as the user would");
    let err = from_outside(&ctx, &mut app, r#"{"op":"click","text":"Nowhere at all"}"#).unwrap_err();
    assert!(err.contains("isn't on screen"), "{err}");
}
