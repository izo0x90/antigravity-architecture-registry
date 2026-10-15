//! Keys and actions: bindings from config, ranking, menus, the palette.

use super::*;

#[test]
fn vim_sequences_and_leader_drive_the_graph_and_are_rebindable() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, key("l"));
    assert_eq!(selected(&app), "rust:a::run");
    app.desk.details.insert(crate::goto::tests::detail("rust:a::run", vec![call("rust:a::run", "rust:a::step", 0, 2), call("rust:a::run", "rust:a::log", 1, 3)], vec![]), &Source::Code);
    frame(&ctx, &mut app, key("g"));
    assert_eq!(app.pending_keys.len(), 1, "waiting for the second key");
    frame(&ctx, &mut app, key("c"));
    assert!(app.pending_keys.is_empty());
    // Two callees: the palette asks which.
    assert_eq!(app.input_mode, InputMode::Palette);
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert_eq!(app.input_mode, InputMode::Normal, "Escape closes the palette");
    frame(&ctx, &mut app, vec![]);
    assert!(!text_input_focused(&ctx), "the palette's field let go of the keyboard");
    app.desk.graph.select(Some("rust:a::run".into()));
    frame(&ctx, &mut app, key("l"));
    assert_eq!(selected(&app), "rust:a::step", "keys work right after the palette");
    // A button holding the keyboard (Tab) doesn't swallow keys either.
    ctx.memory_mut(|m| m.request_focus(egui::Id::new("a button")));
    app.desk.graph.select(Some("rust:a::run".into()));
    frame(&ctx, &mut app, key("l"));
    assert_eq!(selected(&app), "rust:a::step", "keys work while a button has focus");
    assert!(ctx.memory(|m| m.focused()).is_none(), "and the button gave the keyboard back");
    app.desk.details.insert(crate::goto::tests::detail("rust:a::step", vec![call("rust:a::step", "rust:a::save", 0, 5)], vec![]), &Source::Code);
    keys(&ctx, &mut app, "gc");
    assert_eq!(selected(&app), "rust:a::save", "single callee: straight there");
    ctrl(&ctx, &mut app, egui::Key::O);
    assert_eq!(selected(&app), "rust:a::step", "following an edge is in the jump list: back undoes it");
    keys(&ctx, &mut app, "gw");
    assert!(app.pending_keys.is_empty(), "a key that continues nothing drops the sequence");
    // The leader menu lists the graph's own under g.
    let bound = app.bound_actions();
    let entries = leader_entries(&bound, &['g'], Origin::View(BufferKind::Graph));
    assert!(entries.iter().any(|e| matches!(e, LeaderEntry::Action { key: 'f', index } if bound[*index].action.id == "graph.go_in")));
    assert!(entries.iter().any(|e| matches!(e, LeaderEntry::Action { key: 'v', index } if bound[*index].action.id == "graph.view")));
    // Rebinding from config takes effect.
    app.config = UiConfig::with_overrides("[keymap.bind]\n\"nav.right\" = { keys = [\"w\"] }\n").unwrap();
    app.desk.graph.select(Some("rust:a::run".into()));
    frame(&ctx, &mut app, key("w"));
    assert_eq!(selected(&app), "rust:a::step");
}

#[test]
fn tabs_are_icons_that_name_themselves_and_their_key() {
    let (ctx, mut app) = app_with_graph();
    let shell = app.shell_actions();
    let keys = app.tab_keys(&shell);
    let at = |kind: BufferKind| BufferKind::ALL.iter().position(|k| *k == kind).unwrap();
    assert_eq!((keys[at(BufferKind::Graph)].as_str(), keys[at(BufferKind::Queue)].as_str()), ("Space b g", "Space b q"));
    // Only each slot's active tab is named.
    let text = texts(&frame(&ctx, &mut app, vec![]));
    let named = |kind: BufferKind| text.lines().any(|l| l == format!("{} {}{}", kind.icon(), &kind.title()[..1], kind.title()[1..].to_lowercase()));
    let active = app.desk.columns.kinds();
    for kind in BufferKind::ALL {
        assert_eq!(named(kind), active.contains(&kind), "{kind:?}: {text}");
    }
}

#[test]
fn every_key_comes_from_config_and_no_two_actions_in_a_view_share_one() {
    let config = UiConfig::with_overrides("").unwrap();
    let only_in = |id: &str| BufferKind::ALL.into_iter().find(|k| id.starts_with(&format!("{}.", k.name())));
    let label = |seq: &crate::keymap::KeySeq| seq.0.iter().map(Chord::label).collect::<Vec<_>>().join(" ");
    for view in BufferKind::ALL {
        let mut seqs: Vec<(String, &str)> = Vec::new();
        for (id, binding) in &config.keymap.bind {
            if only_in(id).is_some_and(|k| k != view) {
                continue;
            }
            for seq in &binding.keys {
                seqs.push((label(seq), id.as_str()));
            }
        }
        for (i, (a, id_a)) in seqs.iter().enumerate() {
            assert!(!(a.len() == 1 && a.chars().all(|c| c.is_ascii_digit())), "{id_a}: digits are counts in every view");
            for (b, id_b) in &seqs[i + 1..] {
                assert_ne!(a, b, "{id_a} and {id_b} share `{a}` in the {} view", view.name());
                assert!(!b.starts_with(&format!("{a} ")) && !a.starts_with(&format!("{b} ")), "`{a}` ({id_a}) and `{b}` ({id_b}): the shorter can never run in the {} view", view.name());
            }
        }
    }
    // The old clashes are gone: g g is first everywhere, g m is the graph,
    // g r is callers, x reopens in the spec.
    let keys_of = |id: &str| config.keymap.bind.get(id).map(|b| b.keys.iter().map(label).collect::<Vec<_>>()).unwrap_or_default();
    assert_eq!(keys_of("nav.first"), ["g g"]);
    assert_eq!(keys_of("goto.graph"), ["g m"]);
    assert_eq!(keys_of("goto.uses"), ["g r"], "g r lists every use");
    assert_eq!(keys_of("goto.callers"), ["g R"]);
    assert_eq!(keys_of("spec.reopen"), ["x"]);
    for gone in ["spec.show_in_graph", "queue.show_in_graph", "code.reveal", "graph.back", "graph.search", "graph.hint"] {
        assert!(!config.keymap.bind.contains_key(gone), "{gone} is replaced");
    }
    // Live: whatever each view offers, no key runs two things.
    let (ctx, mut app) = full_app();
    app.desk.graph.select(Some("rust:a::run".into()));
    for kind in BufferKind::ALL {
        focus(&ctx, &mut app, kind);
        let bound = app.bound_actions();
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        for b in &bound {
            for seq in &b.keys {
                let k = seq.iter().map(Chord::label).collect::<Vec<_>>().join(" ");
                if let Some(other) = seen.insert(k.clone(), b.action.id.clone()) {
                    panic!("`{k}` runs both {other} and {} in the {} view", b.action.id, kind.name());
                }
            }
        }
    }
}

#[test]
fn actions_rank_by_importance_and_apply_to_the_target() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    let bound = app.bound_actions();
    assert!(bound.iter().all(|b| (PRIMARY..=MINOR).contains(&b.action.importance)), "every action has an importance");
    let bar: Vec<String> = app.bar_keys(&bound).into_iter().map(|b| b.action.id.clone()).collect();
    assert!(bar.len() <= app.config.keymap.bar.max);
    assert!(!bar.iter().any(|id| id.starts_with("goto.")), "nothing selected: no go-tos: {bar:?}");
    assert!(bar.contains(&"nav.hint".to_owned()) && bar.contains(&"nav.search".to_owned()));
    app.desk.graph.select(Some("rust:a::step".into()));
    let bound = app.bound_actions();
    let bar: Vec<&Bound> = app.bar_keys(&bound);
    let ids: Vec<&str> = bar.iter().map(|b| b.action.id.as_str()).collect();
    assert!(ids.contains(&"nav.enter") && ids.contains(&"goto.code"), "{ids:?}");
    assert!(bar.windows(2).all(|w| w[0].action.importance <= w[1].action.importance), "most important first");
    assert_eq!(bar.iter().find(|b| b.action.id == "nav.enter").unwrap().action.label, "Into step", "says what it does to the target");
    assert!(!ids.contains(&"nav.search_next"), "nothing searched yet");
    app.desk.searches.insert(BufferKind::Graph, "s".into());
    assert!(When::Searched.holds(&app.situation()));
    // The bar draws them, keys first; tappable.
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Enter Into step") && text.contains("f Jump to something on screen"), "{text}");
    // Narrow screens get big chips; the bar can be turned off.
    app.config = UiConfig::with_overrides("[keymap.bar]\nshow = false\n").unwrap();
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(!text.contains("Enter Into step"));
    // A spec item: the review verbs lead.
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Spec);
    let bound = app.bound_actions();
    let ids: Vec<String> = app.bar_keys(&bound).into_iter().map(|b| b.action.id.clone()).collect();
    assert!(ids[..4].contains(&"spec.comment".to_owned()), "{ids:?}");
    assert!(!When::Named.holds(&app.situation()), "not a code line");
}

#[test]
fn a_frame_gathers_the_actions_once_and_again_only_after_one_runs() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    let before = app.gathered.get();
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.gathered.get() - before, 1, "once for the key bar, the hints, the palette and the keys");
    // A key that runs an action: gathered for the key, then once more
    // for what it left.
    let before = app.gathered.get();
    frame(&ctx, &mut app, key("j"));
    assert!(app.gathered.get() - before <= 2, "{} times", app.gathered.get() - before);
}

#[test]
fn keys_are_given_out_again_only_when_the_actions_change() {
    let (ctx, mut app) = app_with_graph();
    frame(&ctx, &mut app, vec![]);
    let first = app.bound_actions();
    let kept = kept_keys(&app);
    // Nothing changed: the same keys, not given out again.
    let again = app.bound_actions();
    assert_eq!(kept_keys(&app), kept);
    assert!(first.iter().zip(&again).all(|(a, b)| a.action.id == b.action.id && a.leader == b.leader && a.keys == b.keys));
    // A selection changes what can run: given out again, current labels.
    app.desk.graph.select(Some("rust:a::step".into()));
    let selected = app.bound_actions();
    assert_ne!(kept_keys(&app), kept);
    assert_eq!(selected.iter().find(|b| b.action.id == "nav.enter").unwrap().action.label, "Into step");
    assert!(selected.iter().any(|b| b.action.id == "goto.code" && !b.keys.is_empty()), "the new go-to has its keys");
}

#[test]
fn question_mark_lists_this_views_keys_grouped_and_searchable() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Spec);
    frame(&ctx, &mut app, key("?"));
    assert!(app.key_sheet.is_some());
    let text = texts(&frame(&ctx, &mut app, vec![]));
    for expected in ["SPEC keys", "Navigate", "Go to", "Act", "g d", "Go to its code", "Comment on this item", "J", "Next section"] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert!(!text.contains("Next view"), "only this view's keys (and the shared ones)");
    app.key_sheet = Some("fold".into());
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("View") && text.contains("Fold or unfold") && text.contains("z a"), "{text}");
    app.key_sheet = Some("callee".into());
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Go to a callee") && !text.contains("Half a page down"), "{text}");
    let selection = app.desk.graph.selection.clone();
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    assert!(app.key_sheet.is_none());
    frame(&ctx, &mut app, vec![]);
    assert!(!text_input_focused(&ctx));
    assert_eq!(app.desk.graph.selection, selection, "Escape closed the sheet and nothing else");
}

#[test]
fn a_pause_on_a_prefix_shows_what_can_follow() {
    let (ctx, mut app) = app_with_graph();
    app.desk.graph.select(Some("rust:a::step".into()));
    frame_at(&ctx, &mut app, vec![], 10.0);
    frame_at(&ctx, &mut app, key("g"), 10.0);
    // The popup: its title is the keys so far, and it says how to cancel.
    let popup = |text: &str, prefix: &str| text.contains(&format!("{prefix} …")) && text.contains("Escape to cancel");
    let quick = texts(&frame_at(&ctx, &mut app, vec![], 10.1));
    assert!(!popup(&quick, "g"), "not before the pause");
    frame_at(&ctx, &mut app, vec![], 10.45);
    let text = texts(&frame_at(&ctx, &mut app, vec![], 10.5));
    assert!(popup(&text, "g") && text.contains("Go to its code") && text.contains("Go to a caller") && text.contains("Go to what holds it"), "{text}");
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 10.6);
    assert!(app.pending_keys.is_empty());
    assert!(!popup(&texts(&frame_at(&ctx, &mut app, vec![], 11.5)), "g"), "Escape dismisses it");
    // Typed quickly: never shown.
    frame_at(&ctx, &mut app, key("z"), 20.0);
    frame_at(&ctx, &mut app, key("z"), 20.05);
    assert!(!popup(&texts(&frame_at(&ctx, &mut app, vec![], 21.0)), "z"));
    // The delay is config.
    app.config = UiConfig::with_overrides("[keymap]\nwhich_key_delay_ms = 2000\n").unwrap();
    frame_at(&ctx, &mut app, key("z"), 30.0);
    assert!(!popup(&texts(&frame_at(&ctx, &mut app, vec![], 31.0)), "z"));
    frame_at(&ctx, &mut app, vec![], 32.05);
    let text = texts(&frame_at(&ctx, &mut app, vec![], 32.1));
    assert!(popup(&text, "z") && text.contains("Fold everything"), "{text}");
    // With nothing selected, the go-tos that need a target aren't listed.
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 33.0);
    app.desk.graph.select(None);
    frame_at(&ctx, &mut app, key("g"), 40.0);
    frame_at(&ctx, &mut app, vec![], 42.9);
    let text = texts(&frame_at(&ctx, &mut app, vec![], 43.0));
    assert!(popup(&text, "g") && text.contains("First") && !text.contains("Go to what holds it"), "{text}");
}

#[test]
fn the_palette_still_reaches_other_views_actions() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Graph);
    let bound = app.bound_actions();
    assert!(!bound.iter().any(|b| b.action.id == "spec.pin"), "not bound from the graph");
    let palette = app.palette_actions();
    let pin = palette.iter().find(|b| b.action.id == "spec.pin").expect("in the palette");
    assert!(pin.action.label.starts_with("Spec · ") && pin.keys.is_empty() && pin.leader.is_none());
    let was = app.desk.spec.pinned;
    app.invoke(Invocation { action: pin.action.clone(), origin: pin.origin, value: None }, &ctx);
    assert_ne!(app.desk.spec.pinned, was, "it runs on its view");
    // `:` then a number goes to that line or row.
    assert!(palette.iter().any(|b| b.action.id == "nav.line"));
    focus(&ctx, &mut app, BufferKind::Code);
    let line = app.palette_actions().into_iter().find(|b| b.action.id == "nav.line").unwrap();
    app.invoke(Invocation { action: line.action, origin: line.origin, value: Some("7".into()) }, &ctx);
    assert_eq!(app.desk.code.target_line, Some(7));
}

#[test]
fn the_leader_menu_opens_with_the_focused_views_own_menu() {
    let (ctx, mut app) = full_app();
    for kind in [BufferKind::Code, BufferKind::Spec, BufferKind::Queue] {
        focus(&ctx, &mut app, kind);
        let bound = app.bound_actions();
        let entries = leader_entries(&bound, &[], Origin::View(kind));
        let menu = app.config.keymap.view_menu;
        assert_eq!(entries.first(), Some(&LeaderEntry::Group { key: menu }), "{kind:?}: its own menu first");
        assert!(is_primary(&entries[0], &bound, &[], Origin::View(kind)));
        assert!(!entries[1..].iter().any(|e| is_primary(e, &bound, &[], Origin::View(kind))) || kind == BufferKind::Graph);
        assert!(!entries.contains(&LeaderEntry::Group { key: 'g' }), "{kind:?}: the graph's menu isn't here");
        // Inside it: that view's actions.
        let own = leader_entries(&bound, &[menu], Origin::View(kind));
        assert!(own.iter().all(|e| matches!(e, LeaderEntry::Action { index, .. } if bound[*index].origin == Origin::View(kind))));
    }
    // Drawn: the view's name heads its block.
    focus(&ctx, &mut app, BufferKind::Spec);
    app.input_mode = InputMode::Leader;
    frame(&ctx, &mut app, vec![]);
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("SPEC") && text.contains("All spec actions…"), "{text}");
    frame(&ctx, &mut app, key(","));
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("LEADER > ALL SPEC ACTIONS") && text.contains("Unpin (follow the graph)"), "{text}");
}

#[test]
fn a_code_line_offers_every_go_to() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code.move_to("1");
    assert!(When::Component.holds(&app.situation()), "a line is in some component, even before the index says which");
    frame_at(&ctx, &mut app, key("g"), 70.0);
    frame_at(&ctx, &mut app, vec![], 71.0);
    let text = texts(&frame_at(&ctx, &mut app, vec![], 71.05));
    for expected in ["Show it in the graph", "Go to a caller", "Go to its spec", "Go to its queue job"] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    // No plan: no queue to go to.
    frame_at(&ctx, &mut app, press(egui::Key::Escape, false), 72.0);
    app.desk.queue.plan = None;
    assert!(!app.shell_actions().iter().any(|a| a.id == "goto.queue"));
}

#[test]
fn enter_and_backspace_are_only_offered_where_they_act() {
    let (ctx, mut app) = full_app();
    let offered = |app: &CockpitApp, id: &str| {
        let bound = app.bound_actions();
        app.bar_keys(&bound).iter().any(|b| b.action.id == id)
    };
    // Graph: Backspace only once gone in somewhere.
    focus(&ctx, &mut app, BufferKind::Graph);
    app.desk.graph = showing(crate::graph::tests::tree_of(&crate::graph::tests::sample_graph(), None, arch_graph::tree::TreeRequest { view: arch_graph::tree::ViewKind::Calls, ..Default::default() }));
    app.desk.graph.nav.source = Source::Plan(PlanId::new("p").unwrap());
    app.desk.graph.select(Some("rust:a::run".into()));
    assert!(!app.situation().can_leave, "the starting points already");
    assert!(!offered(&app, "nav.leave"));
    app.desk.graph.navigate(crate::graph::NavChange::GoIn("rust:a::run".into()));
    assert!(app.situation().can_leave);
    // Code: Enter on a call, Backspace inside a definition.
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code.move_to("2");
    assert!(!app.situation().can_enter && !app.situation().can_leave, "run's first line: no call, not inside");
    app.desk.code.move_to("3");
    assert!(app.situation().can_enter && app.situation().can_leave);
    // Queue: Enter only on a job with findings.
    focus(&ctx, &mut app, BufferKind::Queue);
    app.desk.queue.move_to("0");
    assert!(!app.situation().can_enter, "queued, no findings");
    app.desk.queue.move_to("2");
    assert!(app.situation().can_enter);
    // Spec: on a comment, reply leads the bar.
    focus(&ctx, &mut app, BufferKind::Spec);
    let step = ComponentId::new("rust:a::step");
    app.desk.spec.comments = vec![crate::review::tests::comment(1, arch_graph::CommentAnchor::Section { id: step, section: "logic".into() }, arch_graph::CommentStatus::Open)];
    app.desk.spec.move_to("logic#Comment(CommentId(\"c1\"))");
    let bound = app.bound_actions();
    let bar: Vec<String> = app.bar_keys(&bound).into_iter().map(|b| b.action.id.clone()).collect();
    assert_eq!(bar.first().map(String::as_str), Some("spec.reply"), "{bar:?}");
    assert!(!bar.contains(&"spec.comment".to_owned()) || bar.iter().position(|b| b == "spec.reply") < bar.iter().position(|b| b == "spec.comment"));
}

#[test]
fn lists_move_with_the_configured_keys_ctrl_j_and_k_by_default() {
    let (ctx, mut app) = full_app();
    app.open_palette(None);
    frame(&ctx, &mut app, vec![]);
    ctrl(&ctx, &mut app, egui::Key::J);
    ctrl(&ctx, &mut app, egui::Key::J);
    ctrl(&ctx, &mut app, egui::Key::K);
    assert_eq!(app.palette.selected(), 1, "down, down, up");
    // Rebound: the config's keys, not ours.
    app.config = UiConfig::with_overrides("[keymap.list]\nnext = [\"ctrl+e\"]\nprev = [\"ctrl+y\"]\npick = [\"Enter\"]\n").unwrap();
    ctrl(&ctx, &mut app, egui::Key::J);
    assert_eq!(app.palette.selected(), 1, "ctrl+j isn't a key any more");
    ctrl(&ctx, &mut app, egui::Key::E);
    assert_eq!(app.palette.selected(), 2);
}

/// A finger flicked sideways from `from` by `dx`, quickly.
fn swipe(ctx: &egui::Context, app: &mut CockpitApp, from: egui::Pos2, dx: f32, at: f64) {
    let touch = |phase, pos| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(7), phase, pos, force: None };
    let to = from + egui::vec2(dx, 0.0);
    frame_at(ctx, app, vec![touch(egui::TouchPhase::Start, from)], at);
    frame_at(ctx, app, vec![touch(egui::TouchPhase::Move, to)], at + 0.05);
    frame_at(ctx, app, vec![touch(egui::TouchPhase::End, to)], at + 0.1);
    frame_at(ctx, app, vec![], at + 0.15);
}

#[test]
fn a_swipe_runs_what_the_keymap_binds_to_it_in_the_view_it_started_on() {
    let (ctx, mut app) = full_app();
    app.desk.chat = crate::chat::tests::conversation();
    let mut sub = crate::chat::tests::lead(None);
    (sub.id, sub.role, sub.name) = (harness_contracts::SlotId::new("sub-1"), "sub".into(), "reviewer".into());
    app.desk.chat.process_event(harness_contracts::ClientEvent::SlotUpdated { slot: sub });
    focus(&ctx, &mut app, BufferKind::Chat);
    let middle = app.slot_rects[app.desk.columns.focused()].center();
    swipe(&ctx, &mut app, middle, -150.0, 1.0);
    assert_eq!(app.desk.chat.ui.slot.as_str(), "sub-1", "left: the chat's next slot (its own binding before the shell's)");
    swipe(&ctx, &mut app, middle, 150.0, 2.0);
    assert_eq!(app.desk.chat.ui.slot.as_str(), harness_contracts::LEAD, "right: back");
    assert!(UiConfig::with_overrides("[keymap.bind]\n\"chat.next_slot\" = { gestures = [\"wiggle\"] }\n").unwrap_err().contains("no gesture `wiggle`"));
}
