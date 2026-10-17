//! The desk's comparison and what it changes: the files (read again when
//! they change, and now and then while the changes show), which components
//! each change touches, and when the files changed (the timeline). Kept on
//! the desk (`Shared::changes`): the changes view shows it, the code view,
//! the cards and the graph mark it, the chat counts each turn's files.
//! Each part counts the times it was read, so readers take it once.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use harness_contracts::changes::{ChangeList, Comparison, ComponentChangeList, ComponentChanges, Timeline, TurnChanges};

use crate::net::{NetClient, NetResult};

/// How many files one read lists.
const PAGE: usize = 500;

enum Reply {
    List(Comparison, NetResult<ChangeList>),
    Components(Comparison, NetResult<ComponentChangeList>),
    Timeline(NetResult<Timeline>),
}

pub struct Comparing {
    comparison: Comparison,
    list: Option<ChangeList>,
    /// How many times the list was read (readers ask for their files'
    /// changes again when it is).
    read: u64,
    components: Option<ComponentChangeList>,
    components_read: u64,
    timeline: Option<Timeline>,
    timeline_read: u64,
    loading: bool,
    error: Option<String>,
    /// Asked for again (files changed, or the comparison did).
    stale: bool,
    /// When the list was last asked for.
    asked_at: f64,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

impl Default for Comparing {
    fn default() -> Self {
        Self {
            comparison: Comparison::default(),
            list: None,
            read: 0,
            components: None,
            components_read: 0,
            timeline: None,
            timeline_read: 0,
            loading: false,
            error: None,
            stale: true,
            asked_at: f64::NEG_INFINITY,
            inbox: Arc::default(),
        }
    }
}

fn push(inbox: &Arc<Mutex<Vec<Reply>>>, reply: Reply) {
    if let Ok(mut i) = inbox.lock() {
        i.push(reply);
    }
}

impl Comparing {
    pub fn comparison(&self) -> &Comparison {
        &self.comparison
    }

    /// Compares `comparison` from now on (its files read afresh).
    pub fn compare(&mut self, comparison: Comparison) {
        if comparison != self.comparison {
            self.comparison = comparison;
            self.list = None;
            self.stale = true;
            // A list still on its way is for the old comparison: its answer
            // is dropped, so it's not waited for.
            self.loading = false;
        }
    }

    /// Reads it all again (files changed).
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// The list in, if it's for the comparison in use.
    pub fn list(&self) -> Option<&ChangeList> {
        self.list.as_ref().filter(|l| l.comparison == self.comparison)
    }

    /// How many times the list was read.
    pub fn read(&self) -> u64 {
        self.read
    }

    pub fn loading(&self) -> bool {
        self.loading
    }

    /// Why the list couldn't be read, when it couldn't.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Which components it changed, as last read, and how many times that was.
    pub fn components(&self) -> (Option<&ComponentChangeList>, u64) {
        (self.components.as_ref(), self.components_read)
    }

    /// What it changed in each component.
    pub fn by_component(&self) -> BTreeMap<arch_graph::ComponentId, ComponentChanges> {
        self.components.iter().flat_map(|l| &l.components).map(|c| (arch_graph::ComponentId::new(&c.component), c.clone())).collect()
    }

    /// The timeline as last read, and how many times that was.
    pub fn timeline(&self) -> (Option<&Timeline>, u64) {
        (self.timeline.as_ref(), self.timeline_read)
    }

    /// A turn's changes, if the timeline saw it.
    pub fn turn(&self, turn: &str) -> Option<&TurnChanges> {
        self.timeline.as_ref()?.turn(turn)
    }

    /// Applies replies; asks for the list when it's stale, or every
    /// `refresh_s` while `shown` (the index and staging change outside the
    /// files); with each list, the components it touches and the timeline.
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context, shown: bool, refresh_s: f64) {
        let now = ctx.input(|i| i.time);
        let replies = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::List(asked, result) if asked == self.comparison => {
                    self.loading = false;
                    match result {
                        Ok(list) => {
                            self.error = None;
                            self.read += 1;
                            let (inbox, asked) = (self.inbox.clone(), list.comparison.clone());
                            net.change_components(&list.comparison, ctx.clone(), move |r| push(&inbox, Reply::Components(asked, r)));
                            let inbox = self.inbox.clone();
                            net.change_timeline(ctx.clone(), move |r| push(&inbox, Reply::Timeline(r)));
                            self.list = Some(list);
                        }
                        Err(e) => self.error = Some(e.into()),
                    }
                }
                Reply::Components(asked, result) if asked == self.comparison => {
                    // Not there (not a repository, an older server): none.
                    self.components = result.ok();
                    self.components_read += 1;
                }
                Reply::Timeline(Ok(timeline)) => {
                    self.timeline = Some(timeline);
                    self.timeline_read += 1;
                }
                _ => {}
            }
        }
        let due = shown && now - self.asked_at >= refresh_s;
        if (self.stale || due) && !self.loading {
            self.stale = false;
            self.loading = true;
            self.asked_at = now;
            let (inbox, asked) = (self.inbox.clone(), self.comparison.clone());
            net.changes(&self.comparison, 0, PAGE, ctx.clone(), move |r| push(&inbox, Reply::List(asked, r)));
        }
        if shown {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(refresh_s.max(0.5)));
        }
    }

    /// The list, as if it came from the server (tests).
    #[cfg(test)]
    pub(crate) fn take_list_for_test(&mut self, list: ChangeList) {
        self.comparison = list.comparison.clone();
        self.list = Some(list);
        self.read += 1;
    }

    /// The components, as if they came from the server (tests).
    #[cfg(test)]
    pub(crate) fn take_components_for_test(&mut self, components: ComponentChangeList) {
        self.components = Some(components);
        self.components_read += 1;
    }

    /// The timeline, as if it came from the server (tests).
    #[cfg(test)]
    pub(crate) fn take_timeline_for_test(&mut self, timeline: Timeline) {
        self.timeline = Some(timeline);
        self.timeline_read += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::changes::{ChangedFile, FileStatus, Group, Point};

    #[test]
    fn switching_the_comparison_while_a_list_is_on_its_way_asks_for_the_new_one() {
        let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
        let mut desk = Comparing::default();
        desk.tick(&net, &ctx, false, 5.0);
        assert!(desk.loading(), "the first list asked for");
        let staged = Comparison { from: Point::Head, to: Point::Index };
        desk.compare(staged.clone());
        desk.tick(&net, &ctx, false, 5.0);
        let asked = |net: &NetClient| net.sent.lock().unwrap().iter().filter(|s| s.contains("/api/changes?")).count();
        assert_eq!(asked(&net), 2, "the new comparison asked for at once, not after the old answer");
        // The old answer comes late: dropped; the new one is read, and with
        // it the components it touches and the timeline are asked for.
        push(&desk.inbox, Reply::List(Comparison::default(), Ok(ChangeList { comparison: Comparison::default(), files: vec![], offset: 0, total: 0 })));
        desk.tick(&net, &ctx, false, 5.0);
        assert!(desk.loading() && desk.list().is_none() && desk.read() == 0, "still waiting for the new one");
        let file = ChangedFile { path: "a.rs".into(), old_path: None, status: FileStatus::Modified, group: Group::Staged, added: 1, removed: 0, binary: false };
        push(&desk.inbox, Reply::List(staged.clone(), Ok(ChangeList { comparison: staged, files: vec![file], offset: 0, total: 1 })));
        desk.tick(&net, &ctx, false, 5.0);
        assert_eq!((desk.loading(), desk.list().map(|l| l.total), desk.read()), (false, Some(1), 1));
        assert!(net.sent.lock().unwrap().iter().any(|s| s.contains("/api/changes/components")) && net.sent.lock().unwrap().iter().any(|s| s.contains("/api/changes/timeline")));
    }
}
