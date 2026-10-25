//! The chat's stream as rows, what the cursor, the moves and the drawing
//! go by. Each item is a row, except a run of calls (two or more in a row
//! in one turn: tool calls, commands, file changes; quiet, everything
//! between two messages), which is one row while folded ("Read 3 files,
//! searched twice, ran 1 command"). A call's output, and reasoning, fold
//! too, and so does what a call's sub-agent did (its items follow the
//! call, nested a level deeper). Folded or open is by default (the
//! verbosity says), or as everything was set at once (`z R`, `z M`), or
//! as set by hand, by item id: what's set holds as the stream grows and
//! as calls end.

use std::collections::BTreeMap;

use harness_contracts::{ItemBody, ItemId, ItemStatus, TurnItem};

use crate::config::Verbosity;

/// One row: the items it covers (their places in the stream, `first` to
/// `last`); the run it's in (its first and last item), folded into this
/// row or open, each of its items a row; how deep it's nested (within how
/// many calls' sub-agents); how many items its sub-agent's work holds
/// (folded, they're in this row).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub first: usize,
    pub last: usize,
    pub run: Option<(usize, usize)>,
    pub folded: bool,
    pub depth: usize,
    pub nested: usize,
}

impl Row {
    pub(crate) fn holds(&self, index: usize) -> bool {
        (self.first..=self.last).contains(&index)
    }
}

/// The stream as rows, and whether anything in it is folded, anything
/// open (worked out together, once per change).
#[derive(Debug, Default)]
pub(crate) struct Shape {
    pub rows: Vec<Row>,
    pub any_folded: bool,
    pub any_open: bool,
}

impl Shape {
    /// The row holding item `index`, by its place among the rows.
    pub(crate) fn row_of(&self, index: usize) -> Option<usize> {
        let at = self.rows.partition_point(|r| r.last < index);
        self.rows.get(at).filter(|r| r.holds(index)).map(|_| at)
    }
}

/// How a fold was set: open, or folded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Set {
    Open,
    Folded,
}

impl Set {
    fn open(self) -> bool {
        self == Self::Open
    }
}

/// What's folded and open: the verbosity's defaults, everything set at
/// once over them, and what's set by hand over both (runs by their first
/// item's id, an item's own fold by its id).
#[derive(Default, Debug)]
pub(crate) struct Folds {
    verbosity: Verbosity,
    all: Option<Set>,
    runs: BTreeMap<ItemId, Set>,
    items: BTreeMap<ItemId, Set>,
    /// Counts every change (what's worked out from them is kept till then).
    changes: u64,
}

/// A call: a tool call, a command, a file change.
pub(crate) fn is_call(item: &TurnItem) -> bool {
    matches!(item.body, ItemBody::ToolCall { .. } | ItemBody::CommandExecution { .. } | ItemBody::FileChange { .. })
}

/// Whether an item folds on its own: a call with output, a change with
/// its diff, reasoning, your message with what went with it.
pub(crate) fn folds(item: &TurnItem) -> bool {
    item.body.has_detail()
}

impl Folds {
    pub(crate) fn verbosity(&self) -> Verbosity {
        self.verbosity
    }

    pub(crate) fn changes(&self) -> u64 {
        self.changes
    }

    /// Another verbosity: its defaults, with nothing set (the one it is:
    /// nothing changes).
    pub(crate) fn set_verbosity(&mut self, verbosity: Verbosity) {
        if verbosity != self.verbosity {
            self.verbosity = verbosity;
            self.afresh();
        }
    }

    /// Nothing set, the verbosity kept (another thread shown).
    pub(crate) fn afresh(&mut self) {
        *self = Self { verbosity: self.verbosity, changes: self.changes + 1, ..Self::default() };
    }

    /// A fold's default, open or folded (`failed`: it holds a failed call).
    fn open_by_default(&self, failed: bool) -> bool {
        match self.verbosity {
            Verbosity::Quiet => false,
            Verbosity::Normal => failed,
            Verbosity::Everything => true,
        }
    }

    /// Whether a run (its items) shows each of them: as set by hand; open
    /// while you've opened one of its items; else as everything was set,
    /// else by default.
    pub(crate) fn run_open(&self, run: &[&TurnItem]) -> bool {
        let Some(first) = run.first() else { return false };
        if let Some(set) = self.runs.get(&first.id) {
            return set.open();
        }
        if run.iter().any(|c| self.items.get(&c.id) == Some(&Set::Open)) {
            return true;
        }
        self.all.map_or_else(|| self.open_by_default(run.iter().any(|c| c.status == ItemStatus::Failed)), Set::open)
    }

    /// Whether an item's own fold is open.
    pub(crate) fn item_open(&self, item: &TurnItem) -> bool {
        self.items.get(&item.id).copied().or(self.all).map_or_else(|| self.open_by_default(item.status == ItemStatus::Failed), Set::open)
    }

    /// `folding` opened or folded, by hand.
    pub(crate) fn set(&mut self, folding: &Folding, set: Set) {
        match folding {
            Folding::Run { first, .. } => self.set_run(first, set),
            Folding::Item { id, .. } => {
                self.items.insert(id.clone(), set);
                self.changes += 1;
            }
        }
    }

    /// The run starting at item `first` opened or folded, by hand.
    pub(crate) fn set_run(&mut self, first: &ItemId, set: Set) {
        self.runs.insert(first.clone(), set);
        self.changes += 1;
    }

    /// Everything opened or folded at once (what was set by hand goes).
    pub(crate) fn set_all(&mut self, set: Set) {
        self.afresh();
        self.all = Some(set);
    }

    /// The runs in `items` (the stream, in order), first to last: items
    /// in a row in one turn that fold together (calls; quiet, reasoning
    /// too), two or more (quiet, even one).
    /// (A call whose sub-agent did anything stands on its own; a run is
    /// of one sub-agent's calls, or the agent's own.)
    pub(crate) fn runs(&self, items: &[&TurnItem], nesting: &Nesting) -> Vec<(usize, usize)> {
        let quiet = self.verbosity == Verbosity::Quiet;
        let joins = |k: usize| (is_call(items[k]) || quiet && matches!(items[k].body, ItemBody::Reasoning { .. })) && nesting.end[k] == k;
        let together = |a: usize, b: usize| items[a].turn_id == items[b].turn_id && items[a].parent_id == items[b].parent_id;
        let fewest = if quiet { 0 } else { 1 };
        let mut out = Vec::new();
        let mut i = 0;
        while i < items.len() {
            let mut end = i;
            while joins(i) && end + 1 < items.len() && joins(end + 1) && together(i, end + 1) {
                end += 1;
            }
            if joins(i) && end - i >= fewest {
                out.push((i, end));
            }
            i = end + 1;
        }
        out
    }

    /// What folds where the cursor is (on `row`, `first` its first item;
    /// `run`: the items of the run it's in): the run, and the item's own
    /// fold (its output, its reasoning, its sub-agent's work) while it
    /// shows.
    pub(crate) fn here(&self, row: &Row, run: &[&TurnItem], first: &TurnItem) -> Here {
        let run = row.run.and(Folding::of_run(run)).map(|run| (run, !row.folded));
        let what = match &first.body {
            _ if row.nested > 0 => Own::Work,
            ItemBody::Reasoning { .. } => Own::Reasoning,
            ItemBody::UserMessage { .. } => Own::HandedOn,
            _ => Own::Output,
        };
        let own = (!row.folded && (folds(first) || row.nested > 0)).then(|| (Folding::Item { id: first.id.clone(), what }, self.item_open(first)));
        Here { run, own }
    }
}

/// What a fold is: a run (its first item, how many it holds, whether
/// they're all calls), or an item's own (its id, and what it folds).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Folding {
    Run { first: ItemId, calls: usize, only_calls: bool },
    Item { id: ItemId, what: Own },
}

/// What an item's own fold holds: a call's output, the reasoning, what
/// the call's sub-agent did (its output too), or what went with your
/// message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Own {
    Output,
    Reasoning,
    Work,
    HandedOn,
}

impl Folding {
    /// The run of `items` (none: no items).
    pub(crate) fn of_run(items: &[&TurnItem]) -> Option<Self> {
        Some(Self::Run { first: items.first()?.id.clone(), calls: items.len(), only_calls: items.iter().all(|i| is_call(i)) })
    }

    /// It, in words.
    pub(crate) fn words(&self) -> String {
        match self {
            Self::Run { calls, only_calls, .. } => run_words(*calls, *only_calls),
            Self::Item { what: Own::Reasoning, .. } => "the reasoning".to_owned(),
            Self::Item { what: Own::Output, .. } => "its output".to_owned(),
            Self::Item { what: Own::Work, .. } => "its sub-agent's work".to_owned(),
            Self::Item { what: Own::HandedOn, .. } => "what went with it".to_owned(),
        }
    }
}

/// A run of `calls` (`only_calls`: no reasoning among them), in words.
fn run_words(calls: usize, only_calls: bool) -> String {
    match (calls, only_calls) {
        (1, true) => "the call".to_owned(),
        (n, true) => format!("the {n} calls"),
        _ => "what it did here".to_owned(),
    }
}

/// What folds where the cursor is, each with whether it's open: the run
/// it's in, its own (a call's output, reasoning) while it shows. The
/// fold keys act on the innermost: Toggle on a folded run, else its own,
/// else the run; Open on the innermost folded, Close on the innermost
/// open (so `z c` again folds the run).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Here {
    pub run: Option<(Folding, bool)>,
    pub own: Option<(Folding, bool)>,
}

impl Here {
    /// What Enter unfolds: a folded run, or a sub-agent's folded work (a
    /// row standing for several, as a run does).
    pub(crate) fn unfolds(&self) -> Option<&Folding> {
        match (&self.run, &self.own) {
            (Some((run, false)), _) => Some(run),
            (_, Some((own @ Folding::Item { what: Own::Work, .. }, false))) => Some(own),
            _ => None,
        }
    }

    pub(crate) fn toggles(&self) -> Option<&(Folding, bool)> {
        match (&self.run, &self.own) {
            (Some(run @ (_, false)), _) => Some(run),
            (_, Some(own)) => Some(own),
            (run, None) => run.as_ref(),
        }
    }

    pub(crate) fn opens(&self) -> Option<&Folding> {
        [&self.run, &self.own].into_iter().flatten().find(|(_, open)| !open).map(|(f, _)| f)
    }

    pub(crate) fn closes(&self) -> Option<&Folding> {
        [&self.own, &self.run].into_iter().flatten().find(|(_, open)| *open).map(|(f, _)| f)
    }
}

/// How the stream's items nest: each one's depth (within how many calls'
/// sub-agents), and its last descendant (itself: none). A sub-agent's
/// items follow their call's (the store keeps them so).
pub(crate) struct Nesting {
    pub depth: Vec<usize>,
    pub end: Vec<usize>,
}

/// How `items` (the stream, in order) nest.
pub(crate) fn nesting(items: &[&TurnItem]) -> Nesting {
    let mut nest = Nesting { depth: vec![0; items.len()], end: (0..items.len()).collect() };
    // (The items it's within, innermost last: past its parent, done.)
    let mut within: Vec<usize> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let keep = item.parent_id.as_ref().and_then(|p| within.iter().rposition(|w| items[*w].id == *p)).map_or(0, |at| at + 1);
        for done in within.drain(keep..) {
            nest.end[done] = i - 1;
        }
        nest.depth[i] = within.len();
        within.push(i);
    }
    for top in within {
        nest.end[top] = items.len() - 1;
    }
    nest
}

/// The rows of `items` (the stream, in order) as `folds` has them, and
/// whether anything shown is folded or open: in one pass, each run met as
/// the items reach it, a folded sub-agent's work in its call's row.
pub(crate) fn shape(items: &[&TurnItem], folds: &Folds) -> Shape {
    let nest = nesting(items);
    let mut runs = folds.runs(items, &nest).into_iter().peekable();
    let mut shape = Shape { rows: Vec::with_capacity(items.len()), ..Shape::default() };
    let note = |open: bool, shape: &mut Shape| {
        shape.any_folded |= !open;
        shape.any_open |= open;
    };
    let mut i = 0;
    while i < items.len() {
        let depth = nest.depth[i];
        if let Some((a, b)) = runs.next_if(|(a, _)| *a == i) {
            let open = folds.run_open(&items[a..=b]);
            note(open, &mut shape);
            if open {
                for (j, item) in items.iter().enumerate().take(b + 1).skip(a) {
                    if self::folds(item) {
                        note(folds.item_open(item), &mut shape);
                    }
                    shape.rows.push(Row { first: j, last: j, run: Some((a, b)), folded: false, depth, nested: 0 });
                }
            } else {
                shape.rows.push(Row { first: a, last: b, run: Some((a, b)), folded: true, depth, nested: 0 });
            }
            i = b + 1;
            continue;
        }
        let nested = nest.end[i] - i;
        let open = folds.item_open(items[i]);
        if self::folds(items[i]) || nested > 0 {
            note(open, &mut shape);
        }
        let last = if nested > 0 && !open { nest.end[i] } else { i };
        shape.rows.push(Row { first: i, last, run: None, folded: false, depth, nested });
        i = last + 1;
    }
    shape
}

/// What a run did, in words: "Read 3 files, searched twice, ran 1 command
/// (1 failed)".
pub(crate) fn summary(calls: &[&TurnItem]) -> String {
    // (What it did, the thing it did it to; how many.)
    let mut counts: Vec<(&str, Option<&str>, usize)> = Vec::new();
    let mut count = |verb: &'static str, thing: Option<&'static str>| match counts.iter_mut().find(|(v, _, _)| *v == verb) {
        Some((_, _, n)) => *n += 1,
        None => counts.push((verb, thing, 1)),
    };
    for said in calls.iter().filter_map(|c| super::words::of(&c.body)) {
        count(said.did, said.thing);
    }
    let words: Vec<String> = counts
        .into_iter()
        .map(|(verb, thing, n)| match (thing, n) {
            (None, 1) => format!("{verb} once"),
            (None, 2) => format!("{verb} twice"),
            (None, n) => format!("{verb} {n} times"),
            (Some(thing), 1) => format!("{verb} 1 {thing}"),
            (Some(thing), n) => format!("{verb} {n} {thing}s"),
        })
        .collect();
    let mut said = crate::nav::capitalized(&words.join(", "));
    let failed = calls.iter().filter(|c| c.status == ItemStatus::Failed).count();
    if failed > 0 {
        said.push_str(&format!(" ({failed} failed)"));
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::{ContentBlock, ThreadId, ToolKind, TurnId};

    fn item(n: u32, turn: &str, body: ItemBody, status: ItemStatus) -> TurnItem {
        TurnItem { id: ItemId::new(format!("i{n}")), thread_id: ThreadId::new("t"), turn_id: TurnId::new(turn), native_id: None, parent_id: None, ordinal: n, status, title: None, body }
    }

    fn call(n: u32, turn: &str, kind: ToolKind) -> TurnItem {
        item(n, turn, ItemBody::ToolCall { kind, name: "x".into(), input: None, output: vec![ContentBlock::Text { text: "out".into() }], locations: vec![] }, ItemStatus::Completed)
    }

    fn said(n: u32, turn: &str) -> TurnItem {
        item(n, turn, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "hi".into() }], streaming: false }, ItemStatus::Completed)
    }

    fn rows_of(items: &[&TurnItem], folds: &Folds) -> Vec<(usize, usize)> {
        shape(items, folds).rows.iter().map(|r| (r.first, r.last)).collect()
    }

    fn failed(mut item: TurnItem) -> TurnItem {
        item.status = ItemStatus::Failed;
        item
    }

    /// Calls in a row in one turn fold into one row, said in words; a lone
    /// call, or calls across turns, don't; open, each is its own row; all
    /// open or folded at once; the rows found by item.
    #[test]
    fn a_run_of_calls_is_one_row_until_opened() {
        let stream = [said(0, "a"), call(1, "a", ToolKind::Read), call(2, "a", ToolKind::Read), call(3, "a", ToolKind::Search), call(4, "a", ToolKind::Search), call(5, "a", ToolKind::Execute), said(6, "a"), call(7, "a", ToolKind::Read), call(8, "b", ToolKind::Read)];
        let items: Vec<&TurnItem> = stream.iter().collect();
        let mut folds = Folds::default();
        let folded = shape(&items, &folds);
        assert_eq!(rows_of(&items, &folds), [(0, 0), (1, 5), (6, 6), (7, 7), (8, 8)]);
        assert_eq!(summary(&items[1..=5]), "Read 2 files, searched twice, ran 1 command");
        assert_eq!((folded.row_of(3), folded.row_of(6), folded.row_of(9)), (Some(1), Some(2), None));
        assert!(folded.any_folded && !folded.any_open, "the run folded, the outputs inside it unseen");
        folds.set_run(&ItemId::new("i1"), Set::Open);
        assert_eq!(shape(&items, &folds).rows.len(), 9, "open: each call a row");
        folds.set_all(Set::Folded);
        assert_eq!(shape(&items, &folds).rows.len(), 5, "all folded again");
        folds.set_all(Set::Open);
        let open = shape(&items, &folds);
        assert!(open.rows.len() == 9 && open.any_open && !open.any_folded, "all open");
    }

    /// A failed call stays open, and so does the run holding it, by
    /// default; folded by hand, it stays folded.
    #[test]
    fn a_failed_call_stays_open() {
        let stream = [call(1, "a", ToolKind::Read), failed(call(2, "a", ToolKind::Execute))];
        let items: Vec<&TurnItem> = stream.iter().collect();
        let mut folds = Folds::default();
        assert_eq!(shape(&items, &folds).rows.len(), 2, "open");
        assert!(folds.item_open(items[1]) && !folds.item_open(items[0]));
        assert_eq!(summary(&items), "Read 1 file, ran 1 command (1 failed)");
        folds.set_run(&ItemId::new("i1"), Set::Folded);
        assert_eq!(shape(&items, &folds).rows.len(), 1, "folded by hand");
    }

    /// What's opened by hand stays open when a call in it fails (and after
    /// everything was opened at once); a lone call opened by hand stays
    /// open when the next call makes it a run.
    #[test]
    fn what_you_opened_stays_open_as_calls_end_and_come() {
        let mut stream = vec![call(1, "a", ToolKind::Read), call(2, "a", ToolKind::Execute)];
        let mut folds = Folds::default();
        folds.set_run(&ItemId::new("i1"), Set::Open);
        folds.set(&Folding::Item { id: ItemId::new("i2"), what: Own::Output }, Set::Open);
        stream[1] = failed(stream[1].clone());
        let items: Vec<&TurnItem> = stream.iter().collect();
        assert!(shape(&items, &folds).rows.len() == 2 && folds.item_open(items[1]), "still open after it failed");
        let mut all = Folds::default();
        all.set_all(Set::Open);
        assert!(all.run_open(&items) && all.item_open(items[1]));

        let mut lone = vec![said(0, "a"), call(1, "a", ToolKind::Read)];
        let mut folds = Folds::default();
        folds.set(&Folding::Item { id: ItemId::new("i1"), what: Own::Output }, Set::Open);
        lone.push(call(2, "a", ToolKind::Read));
        let items: Vec<&TurnItem> = lone.iter().collect();
        assert_eq!(rows_of(&items, &folds), [(0, 0), (1, 1), (2, 2)], "the run it joined is open: its output still shows");
        assert!(folds.item_open(items[1]));
    }

    /// Where the cursor is in an open run, on a call with output: Toggle
    /// acts on its output, Close on its output while that's open and then
    /// on the run, Open on the innermost folded.
    #[test]
    fn the_fold_keys_act_on_the_innermost_fold() {
        let stream = [call(1, "a", ToolKind::Read), call(2, "a", ToolKind::Read)];
        let items: Vec<&TurnItem> = stream.iter().collect();
        let mut folds = Folds::default();
        folds.set_run(&ItemId::new("i1"), Set::Open);
        let row = |folds: &Folds| shape(&items, folds).rows[1].clone();
        let here = folds.here(&row(&folds), &items, items[1]);
        assert_eq!(here.toggles().map(|(f, open)| (f.words(), *open)), Some(("its output".into(), false)));
        assert_eq!(here.opens().map(Folding::words).as_deref(), Some("its output"));
        assert_eq!(here.closes().map(Folding::words).as_deref(), Some("the 2 calls"), "its output folded: Close folds the run");
        folds.set(here.opens().unwrap(), Set::Open);
        let here = folds.here(&row(&folds), &items, items[1]);
        assert_eq!(here.closes().map(Folding::words).as_deref(), Some("its output"));
        folds.set_run(&ItemId::new("i1"), Set::Folded);
        let row = shape(&items, &folds).rows[0].clone();
        let here = folds.here(&row, &items, items[1]);
        assert!(here.own.is_none() && here.toggles().is_some_and(|(f, open)| f.words() == "the 2 calls" && !open), "folded: the run");
    }

    fn under(mut item: TurnItem, parent: u32) -> TurnItem {
        item.parent_id = Some(ItemId::new(format!("i{parent}")));
        item
    }

    /// A sub-agent's items follow its call's, a level deeper: folded by
    /// default into the call's row ("its sub-agent's work"), each a row
    /// once opened, its calls in runs of their own; a call with work of its
    /// own doesn't join a run with its neighbours.
    #[test]
    fn a_sub_agents_work_folds_under_its_call() {
        let stream = [said(0, "a"), call(1, "a", ToolKind::Read), call(2, "a", ToolKind::Other), under(said(3, "a"), 2), under(call(4, "a", ToolKind::Read), 2), under(call(5, "a", ToolKind::Read), 2), under(call(6, "a", ToolKind::Other), 5), call(7, "a", ToolKind::Read), said(8, "a")];
        let items: Vec<&TurnItem> = stream.iter().collect();
        let nest = nesting(&items);
        assert_eq!((nest.depth.clone(), nest.end.clone()), (vec![0, 0, 0, 1, 1, 1, 2, 0, 0], vec![0, 1, 6, 3, 4, 6, 6, 7, 8]));
        let mut folds = Folds::default();
        let shape_of = |folds: &Folds| shape(&items, folds).rows.iter().map(|r| (r.first, r.last, r.depth, r.nested)).collect::<Vec<_>>();
        assert_eq!(shape_of(&folds), [(0, 0, 0, 0), (1, 1, 0, 0), (2, 6, 0, 4), (7, 7, 0, 0), (8, 8, 0, 0)], "folded into its call's row; the calls around it alone");
        let rows = shape(&items, &folds).rows;
        let here = folds.here(&rows[2], &[], items[2]);
        assert_eq!(here.unfolds().map(Folding::words).as_deref(), Some("its sub-agent's work"));
        folds.set(here.unfolds().unwrap(), Set::Open);
        assert_eq!(shape_of(&folds), [(0, 0, 0, 0), (1, 1, 0, 0), (2, 2, 0, 4), (3, 3, 1, 0), (4, 4, 1, 0), (5, 6, 1, 1), (7, 7, 0, 0), (8, 8, 0, 0)], "open: its items a level in; a call with work of its own alone");
    }

    /// Quiet: everything between two messages is one folded line, even one
    /// call, reasoning too; a failed call doesn't open it. Everything:
    /// every run and output open. Picking the verbosity it has keeps
    /// what's set by hand; another starts afresh.
    #[test]
    fn the_verbosity_says_what_shows_by_default() {
        let thought = item(1, "a", ItemBody::Reasoning { text: "hmm".into(), streaming: false }, ItemStatus::Completed);
        let stream = [said(0, "a"), thought, call(2, "a", ToolKind::Read), failed(call(3, "a", ToolKind::Execute)), said(4, "a"), call(5, "a", ToolKind::Read), said(6, "a")];
        let items: Vec<&TurnItem> = stream.iter().collect();
        let mut quiet = Folds::default();
        quiet.set_verbosity(Verbosity::Quiet);
        let shown: Vec<Option<String>> = shape(&items, &quiet).rows.iter().map(|r| r.folded.then(|| summary(&items[r.first..=r.last]))).collect();
        assert_eq!(shown, [None, Some("Thought once, read 1 file, ran 1 command (1 failed)".into()), None, Some("Read 1 file".into()), None]);
        let mut everything = Folds::default();
        everything.set_verbosity(Verbosity::Everything);
        assert_eq!(shape(&items, &everything).rows.len(), 7, "every one a row");
        assert!(items.iter().filter(|i| folds(i)).all(|i| everything.item_open(i)), "every output open");
        let mut normal = Folds::default();
        assert_eq!(shape(&items, &normal).rows.len(), 7, "normal: the run with a failed call open, each its row");
        normal.set_run(&ItemId::new("i2"), Set::Folded);
        normal.set_verbosity(Verbosity::Normal);
        assert_eq!(shape(&items, &normal).rows.len(), 6, "the same verbosity: what's set stays");
        normal.set_verbosity(Verbosity::Everything);
        assert_eq!(shape(&items, &normal).rows.len(), 7, "another: afresh");
    }
}
