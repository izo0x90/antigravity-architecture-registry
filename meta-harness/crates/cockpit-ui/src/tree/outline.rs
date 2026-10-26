//! The code's outline: what contains what (the graph's Contains view), as a
//! tree source. A row is a component; opening one asks for one level more
//! under it.

use std::sync::{Arc, Mutex};

use arch_graph::tree::{Opened, Tree, TreeRequest, ViewKind};
use arch_graph::{ArchEvent, ComponentId, Source};

use super::source::{RowData, Rows, TreeSource};
use crate::hosted::{Status, Tone};
use crate::nav::Target;
use crate::net::{NetClient, NetResult};
use crate::shared::Shared;

/// An answer, with the number of the request it answers.
type Answer = (u64, NetResult<Tree>);

#[derive(Default)]
pub struct CodeOutline {
    /// The source in use (the code alone, or with a plan).
    source: Source,
    inbox: Arc<Mutex<Vec<Answer>>>,
}

/// The server's Contains tree as rows; a cut says what it left out.
pub(crate) fn rows_of(tree: Tree) -> Rows {
    let cut = tree.cut.as_ref().map(|cut| {
        let why = if cut.reason == arch_graph::tree::CutReason::Time { "out of time" } else { "too big" };
        let mut more =
            format!("It shows {} of {} levels; {} parts left out. Unfold a row to load what's in it.", cut.shown, cut.asked, cut.hidden);
        if cut.starts_left_out > 0 {
            more.push_str(&format!(" {} top-level parts aren't shown.", cut.starts_left_out));
        }
        Status::new(format!("cut ({why})"), Tone::Warn).detail(more)
    });
    let rows =
        tree.cards.into_iter().map(|c| RowData { key: c.key, label: c.label, kind: c.kind, parent: c.parent, more: c.more }).collect();
    Rows { rows, dropped: tree.dropped, cut }
}

impl TreeSource for CodeOutline {
    fn title(&self) -> String {
        "Outline".into()
    }

    fn ask(&mut self, number: u64, opened: &[String], net: &NetClient, ctx: &egui::Context) {
        let request = TreeRequest {
            view: ViewKind::Contains,
            source: self.source.clone(),
            opened: opened.iter().map(|card| Opened { card: card.clone(), levels: 1 }).collect(),
            // With a plan: the plan inside the whole codebase.
            full_code: true,
            ..TreeRequest::default()
        };
        let inbox = self.inbox.clone();
        net.graph_tree(&request, ctx.clone(), move |r| {
            if let Ok(mut i) = inbox.lock() {
                i.push((number, r));
            }
        });
    }

    fn answers(&mut self) -> Vec<(u64, Result<Rows, String>)> {
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        replies.into_iter().map(|(n, r)| (n, r.map(rows_of).map_err(String::from))).collect()
    }

    /// Another source in use: another outline.
    fn share(&mut self, shared: &Shared) -> bool {
        let changed = &self.source != shared.source.get();
        if changed {
            self.source = shared.source.get().clone();
        }
        changed
    }

    /// Changed code or plans.
    fn on_event(&mut self, event: &ArchEvent) -> bool {
        matches!(event, ArchEvent::Changed { .. })
    }

    /// A row is a component.
    fn target(&self, key: &str) -> Option<Target> {
        Some(Target::Component(ComponentId::new(key)))
    }

    fn key_of(&self, target: &Target) -> Option<String> {
        target.component().map(|id| id.as_str().to_owned())
    }
}
