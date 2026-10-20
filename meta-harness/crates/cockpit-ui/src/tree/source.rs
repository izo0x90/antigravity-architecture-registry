//! Where a tree buffer's rows come from: a source answers with rows (each
//! a key, a label, a kind, what holds it, how many parts it has still to
//! load), loading the parts of the rows it's told were opened, and says
//! what a row stands for elsewhere. The tree buffer knows nothing more of
//! it.

use arch_graph::ArchEvent;

use crate::hosted::Status;
use crate::nav::Target;
use crate::net::NetClient;
use crate::shared::Shared;

/// One row, as its source has it.
#[derive(Clone, Debug, PartialEq)]
pub struct RowData {
    pub key: String,
    pub label: String,
    /// What kind of row it is, in a word (shown beside it).
    pub kind: String,
    /// The row holding it (none: a top row).
    pub parent: Option<String>,
    /// Its parts not sent yet (unfolding it loads them).
    pub more: u32,
}

/// A source's answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rows {
    pub rows: Vec<RowData>,
    /// Rows opened that it couldn't open (too much for one answer).
    pub dropped: Vec<String>,
    /// What it left out, in its words.
    pub cut: Option<Status>,
}

pub trait TreeSource {
    /// What the rows are, in a word or two (the buffer's title).
    fn title(&self) -> String;

    /// Asks for the rows, with the parts of the `opened` ones too; the
    /// answer comes back with `number`.
    fn ask(&mut self, number: u64, opened: &[String], net: &NetClient, ctx: &egui::Context);

    /// The answers in since, each with its request's number.
    fn answers(&mut self) -> Vec<(u64, Result<Rows, String>)>;

    /// What the buffers share: true when its rows are to be read again
    /// from scratch.
    fn share(&mut self, _shared: &Shared) -> bool {
        false
    }

    /// The architecture changed on the server: true when its rows are to
    /// be read again.
    fn on_event(&mut self, _event: &ArchEvent) -> bool {
        false
    }

    /// What row `key` stands for elsewhere (a go-to's target).
    fn target(&self, key: &str) -> Option<Target>;

    /// The row a target is, if it has one.
    fn key_of(&self, target: &Target) -> Option<String>;

    /// Where going into row `key` goes instead of unfolding it (a plan's
    /// item: its card); none: it unfolds.
    fn opens(&self, _key: &str) -> Option<crate::nav::GoTo> {
        None
    }

    /// What holds the thing row `key` is (a section: its card, then its
    /// plan), nearest first.
    fn holders(&self, _key: &str) -> Vec<harness_contracts::Subject> {
        Vec::new()
    }
}
