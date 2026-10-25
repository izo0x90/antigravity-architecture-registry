//! The chat's stream as rows, what the cursor, the moves and the drawing
//! go by. Each item is a row, except a run of calls (two or more in a row
//! in one turn: tool calls, commands, file changes), which is one row
//! while folded ("Read 3 files, searched twice, ran 1 command"). A call's
//! output, and reasoning, fold too. Folded or open is by default (a run, a
//! call's output and reasoning folded; a failed call, and a run holding
//! one, open) unless toggled; toggles are kept by item id, so they hold as
//! the stream grows.

use std::collections::BTreeSet;

use harness_contracts::{ItemBody, ItemId, ItemStatus, ToolKind, TurnItem};

/// One row: the items it covers (their places in the stream, `first` to
/// `last`); a run of calls folded into one line, or a single item.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub first: usize,
    pub last: usize,
    /// Folded: what the run did, in words.
    pub run: Option<String>,
}

impl Row {
    pub(crate) fn holds(&self, index: usize) -> bool {
        (self.first..=self.last).contains(&index)
    }
}

/// What's been folded or opened against its default, by item id: runs (by
/// their first call's id), and an item's own fold (a call's output, the
/// reasoning).
#[derive(Default, Debug)]
pub(crate) struct Folds {
    runs: BTreeSet<ItemId>,
    items: BTreeSet<ItemId>,
}

/// A call: a tool call, a command, a file change.
pub(crate) fn is_call(item: &TurnItem) -> bool {
    matches!(item.body, ItemBody::ToolCall { .. } | ItemBody::CommandExecution { .. } | ItemBody::FileChange { .. })
}

/// Whether an item folds on its own: a call with output, reasoning.
pub(crate) fn folds(item: &TurnItem) -> bool {
    match &item.body {
        ItemBody::ToolCall { output, .. } => !output.is_empty(),
        ItemBody::CommandExecution { output, .. } => output.is_some(),
        ItemBody::Reasoning { .. } => true,
        _ => false,
    }
}

impl Folds {
    /// Whether a run (its calls) shows each call.
    pub(crate) fn run_open(&self, calls: &[&TurnItem]) -> bool {
        let failed = calls.iter().any(|c| c.status == ItemStatus::Failed);
        failed != calls.first().is_some_and(|c| self.runs.contains(&c.id))
    }

    /// Whether an item's own fold is open (a failed call's output is).
    pub(crate) fn item_open(&self, item: &TurnItem) -> bool {
        (item.status == ItemStatus::Failed) != self.items.contains(&item.id)
    }

    pub(crate) fn toggle_run(&mut self, first: &ItemId) {
        if !self.runs.remove(first) {
            self.runs.insert(first.clone());
        }
    }

    pub(crate) fn toggle_item(&mut self, item: &ItemId) {
        if !self.items.remove(item) {
            self.items.insert(item.clone());
        }
    }

    /// Everything in `items` open (`open`) or folded: each run and fold
    /// toggled to it where its default isn't.
    pub(crate) fn all(items: &[&TurnItem], open: bool) -> Self {
        let mut all = Self::default();
        for run in runs(items) {
            let calls: Vec<&TurnItem> = run.clone().map(|i| items[i]).collect();
            if all.run_open(&calls) != open {
                all.runs.insert(calls[0].id.clone());
            }
        }
        for item in items.iter().filter(|i| folds(i)) {
            if all.item_open(item) != open {
                all.items.insert(item.id.clone());
            }
        }
        all
    }

    /// What folds at item `at` of `items`: the item's own fold when it has
    /// one, else the run it's in; whether it's open now.
    pub(crate) fn at(&self, items: &[&TurnItem], at: usize) -> Option<(Folding, bool)> {
        let run = runs(items).into_iter().find(|r| r.contains(&at));
        let calls = |r: &std::ops::RangeInclusive<usize>| r.clone().map(|i| items[i]).collect::<Vec<_>>();
        let item = items.get(at)?;
        match run {
            Some(r) if !self.run_open(&calls(&r)) => Some((Folding::Run(items[*r.start()].id.clone(), r.end() - r.start() + 1), false)),
            _ if folds(item) => Some((Folding::Item(item.id.clone(), matches!(item.body, ItemBody::Reasoning { .. })), self.item_open(item))),
            Some(r) => Some((Folding::Run(items[*r.start()].id.clone(), r.end() - r.start() + 1), true)),
            None => None,
        }
    }
}

/// What a fold is: a run (its first call, how many calls), or an item's
/// own (its id; whether it's reasoning, else a call's output).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Folding {
    Run(ItemId, usize),
    Item(ItemId, bool),
}

impl Folding {
    /// It, in words.
    pub(crate) fn words(&self) -> String {
        match self {
            Self::Run(_, n) => format!("the {n} calls"),
            Self::Item(_, true) => "the reasoning".to_owned(),
            Self::Item(_, false) => "its output".to_owned(),
        }
    }
}

impl Folds {
    /// Whether anything in `items` is folded, and whether anything is open.
    pub(crate) fn any(&self, items: &[&TurnItem]) -> (bool, bool) {
        let runs: Vec<bool> = runs(items).into_iter().map(|r| self.run_open(&r.map(|i| items[i]).collect::<Vec<_>>())).collect();
        let own: Vec<bool> = items.iter().filter(|i| folds(i)).map(|i| self.item_open(i)).collect();
        let all = || runs.iter().chain(&own);
        (all().any(|open| !open), all().any(|open| *open))
    }

    pub(crate) fn toggle(&mut self, folding: &Folding) {
        match folding {
            Folding::Run(first, _) => self.toggle_run(first),
            Folding::Item(item, _) => self.toggle_item(item),
        }
    }
}

/// The runs of calls in `items` (the stream, in order): two or more calls
/// in a row in one turn.
pub(crate) fn runs(items: &[&TurnItem]) -> Vec<std::ops::RangeInclusive<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let mut end = i;
        while is_call(items[i]) && end + 1 < items.len() && is_call(items[end + 1]) && items[end + 1].turn_id == items[i].turn_id {
            end += 1;
        }
        if end > i {
            out.push(i..=end);
        }
        i = end + 1;
    }
    out
}

/// The rows of `items` (the stream, in order), as `folds` has them.
pub(crate) fn rows(items: &[&TurnItem], folds: &Folds) -> Vec<Row> {
    let runs = runs(items);
    let mut out = Vec::with_capacity(items.len());
    let mut i = 0;
    while i < items.len() {
        if let Some(run) = runs.iter().find(|r| *r.start() == i) {
            let calls: Vec<&TurnItem> = run.clone().map(|j| items[j]).collect();
            if !folds.run_open(&calls) {
                out.push(Row { first: *run.start(), last: *run.end(), run: Some(summary(&calls)) });
                i = run.end() + 1;
                continue;
            }
        }
        out.push(Row { first: i, last: i, run: None });
        i += 1;
    }
    out
}

/// What a run did, in words: "Read 3 files, searched twice, ran 1 command
/// (1 failed)".
pub(crate) fn summary(calls: &[&TurnItem]) -> String {
    // (What it did, the thing it did it to; how many.)
    let mut counts: Vec<(&str, &str, usize)> = Vec::new();
    let mut count = |verb: &'static str, thing: &'static str| match counts.iter_mut().find(|(v, _, _)| *v == verb) {
        Some((_, _, n)) => *n += 1,
        None => counts.push((verb, thing, 1)),
    };
    for call in calls {
        match &call.body {
            ItemBody::ToolCall { kind, .. } => match kind {
                ToolKind::Read => count("read", "file"),
                ToolKind::Edit => count("edited", "file"),
                ToolKind::Delete => count("deleted", "file"),
                ToolKind::Move => count("moved", "file"),
                ToolKind::Search => count("searched", ""),
                ToolKind::Execute => count("ran", "command"),
                ToolKind::Think => count("thought", ""),
                ToolKind::Fetch => count("fetched", "page"),
                ToolKind::Other => count("used", "tool"),
            },
            ItemBody::CommandExecution { .. } => count("ran", "command"),
            ItemBody::FileChange { .. } => count("changed", "file"),
            _ => {}
        }
    }
    let words: Vec<String> = counts
        .into_iter()
        .map(|(verb, thing, n)| match (thing, n) {
            ("", 1) => format!("{verb} once"),
            ("", 2) => format!("{verb} twice"),
            ("", n) => format!("{verb} {n} times"),
            (thing, 1) => format!("{verb} 1 {thing}"),
            (thing, n) => format!("{verb} {n} {thing}s"),
        })
        .collect();
    let mut said = words.join(", ");
    if let Some(first) = said.get(..1) {
        said = format!("{}{}", first.to_uppercase(), &said[1..]);
    }
    let failed = calls.iter().filter(|c| c.status == ItemStatus::Failed).count();
    if failed > 0 {
        said.push_str(&format!(" ({failed} failed)"));
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::{ContentBlock, ThreadId, TurnId};

    fn item(n: u32, turn: &str, body: ItemBody, status: ItemStatus) -> TurnItem {
        TurnItem { id: ItemId::new(format!("i{n}")), thread_id: ThreadId::new("t"), turn_id: TurnId::new(turn), native_id: None, parent_id: None, ordinal: n, status, title: None, body }
    }

    fn call(n: u32, turn: &str, kind: ToolKind) -> TurnItem {
        item(n, turn, ItemBody::ToolCall { kind, name: "x".into(), input: None, output: vec![ContentBlock::Text { text: "out".into() }], locations: vec![] }, ItemStatus::Completed)
    }

    fn said(n: u32, turn: &str) -> TurnItem {
        item(n, turn, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "hi".into() }], streaming: false }, ItemStatus::Completed)
    }

    /// Calls in a row in one turn fold into one row, said in words; a lone
    /// call, or calls across turns, don't; open, each is its own row.
    #[test]
    fn a_run_of_calls_is_one_row_until_opened() {
        let stream = [said(0, "a"), call(1, "a", ToolKind::Read), call(2, "a", ToolKind::Read), call(3, "a", ToolKind::Search), call(4, "a", ToolKind::Search), call(5, "a", ToolKind::Execute), said(6, "a"), call(7, "a", ToolKind::Read), call(8, "b", ToolKind::Read)];
        let items: Vec<&TurnItem> = stream.iter().collect();
        let mut folds = Folds::default();
        let rows = rows(&items, &folds);
        assert_eq!(rows.iter().map(|r| (r.first, r.last)).collect::<Vec<_>>(), [(0, 0), (1, 5), (6, 6), (7, 7), (8, 8)]);
        assert_eq!(rows[1].run.as_deref(), Some("Read 2 files, searched twice, ran 1 command"));
        folds.toggle_run(&ItemId::new("i1"));
        let open = super::rows(&items, &folds);
        assert_eq!(open.len(), 9, "open: each call a row");
        let folds = Folds::all(&items, false);
        assert_eq!(super::rows(&items, &folds).len(), 5, "all folded again");
        assert_eq!(super::rows(&items, &Folds::all(&items, true)).len(), 9, "all open");
        assert_eq!(folds.at(&items, 3).map(|(f, open)| (f.words(), open)), Some(("the 5 calls".to_owned(), false)), "in a folded run: the run");
    }

    /// A failed call stays open, and so does the run holding it; its
    /// output shows; toggling folds them.
    #[test]
    fn a_failed_call_stays_open() {
        let mut failed = call(2, "a", ToolKind::Execute);
        failed.status = ItemStatus::Failed;
        let stream = [call(1, "a", ToolKind::Read), failed];
        let items: Vec<&TurnItem> = stream.iter().collect();
        let mut folds = Folds::default();
        assert_eq!(rows(&items, &folds).len(), 2, "open");
        assert!(folds.item_open(items[1]) && !folds.item_open(items[0]));
        assert_eq!(summary(&items), "Read 1 file, ran 1 command (1 failed)");
        folds.toggle_run(&ItemId::new("i1"));
        assert_eq!(rows(&items, &folds).len(), 1, "folded by hand");
    }
}
