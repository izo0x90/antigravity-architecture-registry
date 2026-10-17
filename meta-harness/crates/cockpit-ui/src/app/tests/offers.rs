//! Every view, with its cursor on each kind of thing it shows: a shared
//! move that would change something there is offered (in the view's own
//! words, with a key, in the palette), and one offered does something or
//! says why. The test that would have caught folding left out of the key
//! bar.

use super::*;
use crate::nav::{Fold, Move};

/// The moves a view says apply or not where its cursor is (the motions
/// are always offered).
const CHECKED: [Move; 7] = [Move::Enter, Move::Leave, Move::Fold(Fold::Toggle), Move::Fold(Fold::Open), Move::Fold(Fold::Close), Move::Fold(Fold::OpenAll), Move::Fold(Fold::CloseAll)];

/// The app with every view holding something, the Changes view included.
fn app() -> (egui::Context, CockpitApp) {
    let (ctx, mut app) = full_app();
    let fed = crate::changes::tests::view_with_changes();
    (*app.desk.changes_mut(), app.desk.shared.changes) = (fed.view, fed.shared.changes);
    (ctx, app)
}

/// What can be seen: where every view is and what it's on, which has the
/// keyboard, the quick look, the jump list and a go-to under way, and
/// every action offered with its label and whether it applies (a fold done
/// says Unfold after).
fn seen(app: &CockpitApp) -> String {
    let views: Vec<String> = BufferKind::ALL.iter().map(|k| format!("{:?} {:?}", app.desk.view(*k).place(), app.desk.view(*k).target())).collect();
    let bound: Vec<String> = app.bound_actions().iter().map(|b| format!("{}={}:{}", b.action.id, b.action.label, b.action.when.holds(&app.situation()))).collect();
    format!("{:?} {} {} {} {:?} {}", app.focused_kind(), views.join(";"), app.quick_look.is_some(), app.desk.pending_goto.is_some(), app.desk.jumps, bound.join("|"))
}

/// Why nothing happened, if anything said so.
fn said(app: &CockpitApp) -> Option<String> {
    app.note.clone().or_else(|| app.desk.graph().note.clone())
}

fn at(kind: BufferKind, key: &str) -> (egui::Context, CockpitApp) {
    let (ctx, mut app) = app();
    focus(&ctx, &mut app, kind);
    app.desk.view_mut(kind).move_to(key);
    frame(&ctx, &mut app, vec![]);
    (app.note, app.desk.graph_mut().note) = (None, None);
    (ctx, app)
}

#[test]
fn every_view_offers_what_acts_where_its_cursor_is_and_nothing_that_doesnt() {
    let mut problems = Vec::new();
    for kind in BufferKind::ALL {
        let (ctx, app) = app();
        let mut app = app;
        focus(&ctx, &mut app, kind);
        let keys: Vec<String> = app.desk.view(kind).items().into_iter().map(|i| i.key).collect();
        // Up to a dozen places, spread over what it shows.
        let step = (keys.len() / 12).max(1);
        for key in keys.iter().step_by(step) {
            let (ctx, mut app) = at(kind, key);
            for m in CHECKED {
                let before = seen(&app);
                let bound = app.bound_actions().into_iter().find(|b| b.action.id == m.id() && b.origin == Origin::View(kind));
                let applies = bound.as_ref().is_some_and(|b| b.action.when.holds(&app.situation()));
                if let Some(b) = bound.as_ref().filter(|_| applies) {
                    if b.key_hint().is_empty() {
                        problems.push(format!("{kind:?} at {key}: {} has no key", b.action.id));
                    }
                    // Where you are, so the key bar and the sheet's first sections show it.
                    if b.layer >= crate::actions::Layer::Host {
                        problems.push(format!("{kind:?} at {key}: {} is in the {:?} layer, not where you are", b.action.id, b.layer));
                    }
                    if ["Fold or unfold", "Fold", "Unfold"].contains(&b.action.label.as_str()) && matches!(m, Move::Fold(_)) {
                        problems.push(format!("{kind:?} at {key}: {} says \"{}\", not what it folds", b.action.id, b.action.label));
                    }
                    if !app.palette_actions().iter().any(|p| p.action.id == b.action.id) {
                        problems.push(format!("{kind:?} at {key}: {} isn't in the palette", b.action.id));
                    }
                }
                app.run_move(kind, m, None, &ctx);
                frame(&ctx, &mut app, vec![]);
                let changed = seen(&app) != before;
                match (applies, changed, said(&app)) {
                    (true, false, None) => problems.push(format!("{kind:?} at {key}: {} offered but did nothing and said nothing", m.id())),
                    (false, true, _) => problems.push(format!("{kind:?} at {key}: {} acted but wasn't offered", m.id())),
                    _ => {}
                }
                if changed || said(&app).is_some() {
                    (_, app) = at(kind, key);
                }
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
