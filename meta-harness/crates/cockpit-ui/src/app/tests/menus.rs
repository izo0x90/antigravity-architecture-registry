//! The menus: on a phone, a panel's groups wrap inside its width and a tap
//! opens one; one with nothing usable isn't offered; the which-key menu by
//! tap and by key; a long press on a menu's button; the top bar's path.

use super::*;

#[test]
fn on_a_phone_the_groups_wrap_inside_the_panel_and_a_tap_opens_one() {
    let (ctx, mut app) = full_app();
    app.desk.panels = panels_of(&[BufferKind::Chat], 0);
    let phone = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(390.0, 844.0));
    let mut t = 10.0;
    let mut at = |ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>| {
        t += 0.05;
        run(ctx, app, egui::RawInput { events, time: Some(t), screen_rect: Some(phone), ..Default::default() })
    };
    at(&ctx, &mut app, vec![]);
    let output = at(&ctx, &mut app, vec![]);
    let groups = ["Chat actions", "Session", "Buffer", "Panels"];
    let mut rows = Vec::new();
    for group in groups {
        let drawn = *drawn_containing(&output, group).first().unwrap_or_else(|| panic!("{group} isn't drawn"));
        assert!(phone.contains_rect(drawn), "{group}: {drawn:?} on {phone:?}");
        rows.push(drawn.top().round());
    }
    rows.dedup();
    assert!(rows.len() > 1, "wrapped onto more than one row: {rows:?}");
    let session = *drawn_containing(&output, "Session  Space s").first().expect("the Session menu's button");
    let touch =
        |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos: session.center(), force: None };
    let button = |pressed| egui::Event::PointerButton {
        pos: session.center(),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    at(&ctx, &mut app, vec![touch(egui::TouchPhase::Start), egui::Event::PointerMoved(session.center()), button(true)]);
    at(&ctx, &mut app, vec![touch(egui::TouchPhase::End), button(false), egui::Event::PointerGone]);
    at(&ctx, &mut app, vec![]);
    assert_eq!((app.input_mode, app.leader_path.clone()), (InputMode::Leader, vec!['s']), "a tap opens its menu");
    let output = at(&ctx, &mut app, vec![]);
    let entries = drawn_containing(&output, "New session");
    assert!(!entries.is_empty() && entries.iter().all(|r| phone.contains_rect(*r)), "its entries on the screen: {entries:?}");
}

/// A menu's path is long: the top bar's mode is cut short in its place,
/// so the bar keeps its height and its right side doesn't move or get
/// drawn over, at a width with little room between them.
#[test]
fn a_menus_path_in_the_top_bar_moves_nothing() {
    let (ctx, mut app) = full_app();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1300.0, 1000.0));
    let at = |ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>| {
        run(ctx, app, egui::RawInput { events, screen_rect: Some(screen), ..Default::default() })
    };
    at(&ctx, &mut app, vec![]);
    let normal = at(&ctx, &mut app, vec![]);
    let first = |output: &egui::FullOutput, words: &str| {
        *drawn_containing(output, words).first().unwrap_or_else(|| panic!("{words:?} isn't drawn"))
    };
    let (panels, group) = (first(&normal, "Panels: "), first(&normal, "Chat actions"));
    assert!(first(&normal, "NORMAL").max.x < panels.min.x, "the mode before the right side");
    app.open_menu(&[app.config.keymap.view_menu], &ctx);
    let menu = at(&ctx, &mut app, vec![]);
    assert_eq!(app.input_mode, InputMode::Leader);
    let mode = drawn_containing(&menu, "Space")
        .into_iter()
        .filter(|r| r.max.y < group.min.y)
        .min_by(|a, b| a.min.x.total_cmp(&b.min.x))
        .expect("the mode, on the top bar's row");
    assert!(mode.max.x < panels.min.x, "cut short before the right side: {mode:?}, {panels:?}");
    assert_eq!(
        (first(&menu, "Panels: "), first(&menu, "Chat actions")),
        (panels, group),
        "the right side and the panels stay where they were"
    );
}

/// A menu shows on a panel's top line only while something in it can be
/// used there: the quick look's, on a card, not on nothing.
#[test]
fn a_menu_with_nothing_usable_here_isnt_offered() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    assert!(!drawn_containing(&frame(&ctx, &mut app, vec![]), "Quick look  Space q").is_empty(), "on a card");
    app.desk.graph_mut().select(None);
    frame(&ctx, &mut app, vec![]);
    assert!(drawn_containing(&frame(&ctx, &mut app, vec![]), "Quick look").is_empty(), "on nothing");
}

/// The which-key menu by tap and by key: an entry (its next key and its
/// label, one target) runs when tapped; Escape, or its button, goes back
/// one level, and from the top closes it.
#[test]
fn a_which_key_entry_runs_when_tapped_and_escape_goes_back_one_level() {
    let (ctx, mut app) = app_with_graph();
    let menu = app.config.keymap.view_menu;
    app.open_menu(&[menu], &ctx);
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    let text = texts(&output);
    assert!(
        text.lines().zip(text.lines().skip(1)).any(|(key, label)| key.chars().count() == 1 && label == "Show the legend"),
        "an entry says its next key alone: {text}"
    );
    let was = app.desk.graph().legend_open;
    click_on(&ctx, &mut app, "Show the legend");
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.desk.graph().legend_open, app.input_mode), (!was, InputMode::Normal), "tapped: it ran, and the menu closed");

    app.open_menu(&[menu], &ctx);
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!((app.input_mode, app.leader_path.clone()), (InputMode::Leader, Vec::new()), "Escape: back one level");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!(app.input_mode, InputMode::Normal, "from the top: closed");
    app.open_menu(&[menu], &ctx);
    frame(&ctx, &mut app, vec![]);
    click_on(&ctx, &mut app, "Escape");
    assert_eq!((app.input_mode, app.leader_path.clone()), (InputMode::Leader, Vec::new()), "its button: back one level");
}

/// A finger held still on a menu's button says what it is and its keys,
/// as hovering it does (egui's tip, shown while held).
#[test]
fn a_finger_held_on_a_menu_button_says_what_it_is_and_its_keys() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    let button = drawn_containing(&output, "Graph actions  Space ,").first().expect("the view's menu button").center();
    let mut t = 10.0;
    let mut at = |ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>| {
        t += 0.1;
        frame_at(ctx, app, events, t)
    };
    let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos: button, force: None };
    at(
        &ctx,
        &mut app,
        vec![
            touch(egui::TouchPhase::Start),
            egui::Event::PointerMoved(button),
            egui::Event::PointerButton { pos: button, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        ],
    );
    let mut said = String::new();
    for _ in 0..12 {
        said = texts(&at(&ctx, &mut app, vec![]));
    }
    assert!(said.contains("Graph actions (Space ,)"), "held: its tip: {said}");
}

/// The which-key menu is where it stays from the first frame it's shown
/// (it was drawn aside for a frame, measured narrower than it is).
#[test]
fn the_which_key_menu_doesnt_move_once_shown() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    app.open_menu(&[app.config.keymap.view_menu], &ctx);
    let mut shown = Vec::new();
    for _ in 0..4 {
        let output = frame(&ctx, &mut app, vec![]);
        shown.extend(drawn_containing(&output, "Show the legend"));
    }
    assert!(shown.len() >= 3 && shown.windows(2).all(|w| w[0] == w[1]), "the same place each frame: {shown:?}");
}

/// The top bar's Menus button opens the leader's own menu, at its top.
#[test]
fn the_menus_button_opens_the_leaders_own_menu() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    click_on(&ctx, &mut app, "Menus");
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.input_mode, app.leader_path.clone(), app.palette.is_open()), (InputMode::Leader, Vec::new(), false));
}

/// A menu tapped while something else has the keys: each leaves as its
/// way back out leaves it. A search being typed is given up (the cursor
/// back where it was); the action list's half-done start is dropped.
#[test]
fn a_menu_tapped_ends_what_had_the_keys_its_own_way() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    frame(&ctx, &mut app, vec![]);
    let search = |ctx: &egui::Context, app: &mut CockpitApp| {
        frame(ctx, app, key("/"));
        for c in "step".chars() {
            frame(ctx, app, key(&c.to_string()));
        }
    };
    // Its own way back out first: where giving it up leaves the cursor.
    search(&ctx, &mut app);
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    let before = app.desk.view(BufferKind::Code).cursor_key();
    search(&ctx, &mut app);
    assert!(
        matches!(app.capture, Some(Capture::Search(_))) && app.desk.view(BufferKind::Code).cursor_key() != before,
        "searching: the cursor on what it found"
    );
    app.open_menu(&[app.config.keymap.view_menu], &ctx);
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert!(app.capture.is_none() && app.input_mode == InputMode::Normal);
    assert_eq!(app.desk.view(BufferKind::Code).cursor_key(), before, "given up: the cursor back where it was");

    // The list open at a value: a menu over it drops what it held.
    let find = app.palette_actions().into_iter().find(|b| b.action.id == "graph.find").unwrap();
    app.open_palette(&find);
    frame(&ctx, &mut app, vec![]);
    app.open_menu(&[], &ctx);
    assert!(!app.palette.is_open() && app.input_mode == InputMode::Leader);
}

/// A setting's list takes the keys only from where the panel's keys run;
/// from a capture (a mark asked for), the capture keeps them.
#[test]
fn a_settings_list_takes_the_keys_only_from_where_the_panels_keys_run() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, key("m"));
    assert!(matches!(app.capture, Some(Capture::MarkSet)), "{:?}", app.capture);
    let mode = app.input_mode;
    click_on(&ctx, &mut app, "calls");
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.input_mode, mode, "the capture keeps the keys");
}

/// Two panels of one kind are judged each on its own: the graph on a card
/// offers the quick look; the one beside it, on nothing, doesn't.
#[test]
fn two_panels_of_one_kind_are_judged_each_on_its_own() {
    let (ctx, mut app) = app_with_graph();
    app.desk.panels = crate::panels::Panels::new(vec![BufferId::new(BufferKind::Graph, 0), BufferId::new(BufferKind::Graph, 1)], 0);
    frame(&ctx, &mut app, vec![]);
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    let looks: Vec<egui::Rect> =
        drawn_rects(&output, "Quick look").into_iter().chain(drawn_containing(&output, "Quick look  Space q")).collect();
    let in_panel = |i: usize| looks.iter().any(|r| app.panel_rects[i].contains(r.center()));
    assert!(in_panel(0) && !in_panel(1), "only on the card's graph: {looks:?}");
}

/// The chat with no session yet: its banner says so, and its ways out
/// are under it, drawn there by its host.
#[test]
fn the_chats_banner_has_its_buttons_under_it() {
    let (ctx, mut app) = app_with_graph();
    app.desk.panels = panels_of(&[BufferKind::Chat], 0);
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    let banner = *drawn_containing(&output, "No session yet")
        .first()
        .or(drawn_containing(&output, "Waiting for the server").first())
        .expect("a banner");
    let under = drawn_containing(&output, "New session")
        .into_iter()
        .filter(|r| r.top() > banner.bottom())
        .min_by(|a, b| a.top().total_cmp(&b.top()));
    assert!(under.is_some_and(|r| r.top() - banner.bottom() < 60.0), "New session under {banner:?}: {under:?}: {}", texts(&output));
}

/// A finger held on the quick look's top line says what's there, and
/// that's all: the view under it isn't touched.
#[test]
fn a_finger_held_on_the_quick_looks_top_line_reaches_nothing_under_it() {
    let (ctx, mut app) = app_with_graph();
    crate::graph::tests::with_detail(app.desk.graph_mut(), "rust:a::step", "src/a.rs", 10, 20);
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    frame_at(&ctx, &mut app, vec![], 1.0);
    peek_key(&ctx, &mut app, true, 1.0);
    peek_key(&ctx, &mut app, false, 1.05);
    let output = frame_at(&ctx, &mut app, vec![], 1.1);
    let look = ctx.memory(|m| m.area_rect(egui::Id::new("quick_look"))).expect("open");
    let at = drawn_containing(&output, "Spec actions")
        .into_iter()
        .find(|r| look.contains(r.center()))
        .expect("the look's own menu button")
        .center();
    let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos: at, force: None };
    let before = (app.desk.spec().cursor, app.desk.graph().selection.clone());
    frame_at(&ctx, &mut app, vec![touch(egui::TouchPhase::Start)], 1.2);
    frame_at(&ctx, &mut app, vec![], 1.9);
    frame_at(&ctx, &mut app, vec![touch(egui::TouchPhase::End)], 2.0);
    frame_at(&ctx, &mut app, vec![], 2.1);
    assert!(!app.palette.is_open() && app.quick_look.is_some(), "no list opened over it");
    assert_eq!((app.desk.spec().cursor, app.desk.graph().selection.clone()), before, "nothing under it moved");
}

/// A menu entry is one thing to tap: its whole cell (past its words too);
/// under a finger, a finger's height.
#[test]
fn a_menu_entry_is_one_target_a_fingers_height_under_one() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    app.open_menu(&[], &ctx);
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    let session = *drawn_containing(&output, "Session…").first().unwrap_or_else(|| panic!("the Session group's entry: {}", texts(&output)));
    // Past its words, in its cell.
    click(&ctx, &mut app, egui::pos2(session.right() + 30.0, session.center().y));
    assert_eq!((app.input_mode, app.leader_path.clone()), (InputMode::Leader, vec!['s']), "tapped past its words: opened");
    // An entry's height: a line's for a mouse, a finger's under one.
    let tall = |fingers| {
        let mut height = 0.0;
        let _ = egui::Context::default().run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                height = crate::actions::entry(ui, "n", "New session", crate::actions::Emphasis::Plain, 200.0, fingers, "").rect.height();
            });
        });
        height
    };
    assert!(tall(crate::actions::Fingers::No) < 30.0 && tall(crate::actions::Fingers::Yes) >= 40.0);
}
