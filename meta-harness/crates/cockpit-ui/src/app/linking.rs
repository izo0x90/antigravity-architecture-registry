//! Links put where text is written: a tag (`#`, its picker in tagging) or
//! a thing (`@`: a file, lines of one, a symbol, found in the code index).
//! What's picked goes into the box being written in as a link, the same
//! way whichever box it is: the palette's writing set aside, or the
//! focused view's (`Navigable::typing_box`).

use harness_contracts::{Include, Linked, Subject};

use super::*;
use crate::actions::{Action, Choice, Group, Param, When};

/// How many of each kind of hit the `@` picker shows.
const FOUND_EACH: usize = 12;

/// The code index's answers, each with the query it was for.
type Answers = std::sync::Arc<std::sync::Mutex<Vec<(String, crate::net::NetResult<arch_search::Found>)>>>;

/// What the `@` picker found for what was typed, as the server said.
#[derive(Default)]
pub struct Finding {
    /// The query asked about (none: nothing yet), and whether both answers came.
    asked: Option<String>,
    answers: usize,
    found: Vec<Choice>,
    inbox: Answers,
}

/// How long a box's links are left still before they're checked.
const CHECK_AFTER: f64 = 0.4;

/// The server's answers: each box's key, the message checked, what it said.
type Checked = std::sync::Arc<std::sync::Mutex<Vec<(String, harness_contracts::Message, Vec<String>)>>>;

/// What each box's links come to, as the server said (a box with one
/// linked whole): checked a moment after its last change.
#[derive(Default)]
pub struct Checks {
    each: std::collections::BTreeMap<String, Check>,
    inbox: Checked,
}

#[derive(Default)]
struct Check {
    /// The message last seen, when it last changed, whether it's asked.
    message: harness_contracts::Message,
    since: f64,
    asked: bool,
    notes: Vec<String>,
}

impl Checks {
    /// What box `key`'s `message` comes to (none: nothing to say).
    fn tend(&mut self, key: String, message: harness_contracts::Message, now: f64, net: &crate::net::NetClient, ctx: &egui::Context) -> Option<String> {
        for (box_key, said, notes) in std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner)) {
            if let Some(check) = self.each.get_mut(&box_key).filter(|c| c.message == said) {
                check.notes = notes;
            }
        }
        // Only what's linked whole can be too much.
        if !message.links.iter().any(|l| l.include == Include::Whole) {
            self.each.remove(&key);
            return None;
        }
        let check = self.each.entry(key.clone()).or_default();
        if check.message != message {
            *check = Check { message, since: now, asked: false, notes: Vec::new() };
        }
        if !check.asked {
            if now - check.since >= CHECK_AFTER {
                check.asked = true;
                let (inbox, said) = (self.inbox.clone(), check.message.clone());
                net.check_links(&check.message, ctx.clone(), move |r| {
                    if let (Ok(notes), Ok(mut i)) = (r, inbox.lock()) {
                        i.push((key, said, notes));
                    }
                });
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(CHECK_AFTER));
            }
        }
        (!check.notes.is_empty()).then(|| check.notes.join(" "))
    }
}

/// `path:10-20` (or `path:10`): lines of a file.
fn lines_of(query: &str) -> Option<(String, u32, u32)> {
    let (path, range) = query.trim().rsplit_once(':')?;
    let (first, last) = match range.split_once('-') {
        Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
        None => {
            let n = range.parse().ok()?;
            (n, n)
        }
    };
    (!path.is_empty() && first >= 1 && last >= first).then(|| (path.to_owned(), first, last))
}

fn choice(subject: &Subject, label: String, detail: String, icon: &'static str) -> Choice {
    Choice::new(serde_json::to_string(subject).unwrap_or_default(), label).detail(Some(detail)).glyph(icon, crate::theme::colors().accent)
}

impl CockpitApp {
    /// What `@` opens: the code's files and symbols (lines with `path:a-b`).
    pub(super) fn link_actions(&self, here: Here) -> Vec<Action> {
        let picking = self.palette.picking() == Some("link.thing");
        let found = if picking { self.desk.finding.found.clone() } else { Vec::new() };
        let waiting = picking && self.desk.finding.answers < 2 && self.desk.finding.asked.is_some();
        let writing = self.desk.view(here.kind).takes_typed() || self.palette.is_writing() || self.palette.holding();
        vec![
            Action::new("link.thing", "Link a file or a symbol", "Where you're writing: the agent is handed it (whole when small, else its start; the link's key changes that)", ShellCommand::LinkThing)
                .param(Param::Search { hint: "a file or a symbol (path:10-20: those lines)".into(), found, waiting })
                .group(Group::Act)
                .importance(crate::actions::MINOR)
                .when(if writing { When::Always } else { When::Unavailable }),
        ]
    }

    /// Asks the code index for what the `@` picker's query finds, once per
    /// query; takes the answers.
    pub(super) fn find_links(&mut self, ctx: &egui::Context) {
        let answers: Vec<_> = std::mem::take(&mut *self.desk.finding.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for (query, answer) in answers {
            if self.desk.finding.asked.as_deref() != Some(query.as_str()) {
                continue;
            }
            self.desk.finding.answers += 1;
            match answer {
                Ok(found) => {
                    let hits = found.hits.into_iter().filter_map(|hit| match found.kind {
                        arch_search::SearchKind::Files => {
                            let path = hit.file.clone().unwrap_or_else(|| hit.label.clone());
                            Some(choice(&Subject::CodeFile { path: path.clone() }, path, "a file".into(), egui_phosphor::regular::FILE))
                        }
                        arch_search::SearchKind::Symbols => {
                            let id = arch_graph::ComponentId(hit.key.clone()?);
                            Some(choice(&Subject::Component { id, was: None }, hit.label.clone(), hit.detail.clone(), egui_phosphor::regular::CODE))
                        }
                        arch_search::SearchKind::Text => None,
                    });
                    self.desk.finding.found.extend(hits);
                }
                Err(e) => self.note = Some(format!("Couldn't search the code: {}", String::from(e))),
            }
        }
        if self.palette.picking() != Some("link.thing") {
            return;
        }
        let query = self.desk.shell_searches.get("link.thing").cloned().unwrap_or_default();
        if self.desk.finding.asked.as_deref() == Some(query.as_str()) {
            return;
        }
        self.desk.finding = Finding { asked: Some(query.clone()), inbox: self.desk.finding.inbox.clone(), ..Finding::default() };
        if let Some((path, first, last)) = lines_of(&query) {
            let words = if first == last { format!("{path} line {first}") } else { format!("{path} lines {first}-{last}") };
            self.desk.finding.found.push(choice(&Subject::CodeLines { path, first, last, snapshot: String::new() }, words, "lines of a file".into(), egui_phosphor::regular::LIST_NUMBERS));
        }
        if query.trim().is_empty() {
            self.desk.finding.answers = 2;
            return;
        }
        let source = self.desk.shared.source.get().clone();
        for kind in [arch_search::SearchKind::Files, arch_search::SearchKind::Symbols] {
            let (inbox, asked) = (self.desk.finding.inbox.clone(), query.clone());
            self.desk.net.search(&query, kind, &source, FOUND_EACH, None, ctx.clone(), move |r| {
                if let Ok(mut i) = inbox.lock() {
                    i.push((asked, r));
                }
            });
        }
    }

    /// Puts a link where text is being written: the palette's writing set
    /// aside, else the box of the view at `here`; back to writing there.
    pub(super) fn put_link(&mut self, here: Here, label: &str, to: Linked) {
        if self.palette.holding() {
            if let Some(writing) = self.palette.put_back() {
                writing.put_link(label, to, Include::Auto);
            }
            self.input_mode = InputMode::Palette;
            return;
        }
        if let Some(writing) = self.desk.view_mut(here.kind).typing_box() {
            writing.put_link(label, to, Include::Auto);
        }
        self.input_mode = InputMode::Insert { at: here.at };
    }

    /// Every box being written in (the views', the palette's): its links in
    /// their colours (a tag's own), and, when one is linked whole, what
    /// they come to checked with the server before it's sent (a moment
    /// after the last change), said under the box.
    pub(super) fn tend_links(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let colour = {
            let tags = self.desk.shared.tags.get();
            let colours: std::collections::BTreeMap<harness_contracts::TagId, egui::Color32> = tags.list().iter().map(|t| (t.id.clone(), crate::theme::colors().tag(t.colour))).collect();
            move |to: &Linked| match to {
                Linked::Tag { tag } => colours.get(tag).copied().unwrap_or(crate::theme::colors().accent),
                Linked::Thing { .. } => crate::theme::colors().accent,
            }
        };
        // Coloured, and what each holds read; then checked; then said.
        let mut written: Vec<(String, harness_contracts::Message)> = Vec::new();
        self.desk.each_buffer_by_id(|id, buffer| {
            if let Some(writing) = buffer.typing_box() {
                writing.colour_links(&colour);
                written.push((format!("{id:?}"), writing.message()));
            }
        });
        if let Some(writing) = self.palette.writing_box() {
            writing.colour_links(&colour);
            written.push(("palette".into(), writing.message()));
        }
        let warnings: std::collections::BTreeMap<String, Option<String>> = written.into_iter().map(|(key, message)| (key.clone(), self.desk.checks.tend(key, message, now, &self.desk.net, ctx))).collect();
        self.desk.each_buffer_by_id(|id, buffer| {
            if let Some(writing) = buffer.typing_box() {
                writing.warning = warnings.get(&format!("{id:?}")).cloned().flatten();
            }
        });
        if let Some(writing) = self.palette.writing_box() {
            writing.warning = warnings.get("palette").cloned().flatten();
        }
    }

    /// The `@` picker's pick: linked where it was opened from.
    pub(super) fn run_link(&mut self, value: Option<String>) {
        let here = self.here();
        let Some(subject) = value.and_then(|v| serde_json::from_str::<Subject>(&v).ok()) else { return };
        let label = self.desk.finding.found.iter().find(|c| serde_json::from_str::<Subject>(&c.value).ok().as_ref() == Some(&subject)).map_or_else(|| "something".to_owned(), |c| c.label.clone());
        self.desk.shell_searches.remove("link.thing");
        self.desk.finding = Finding { inbox: self.desk.finding.inbox.clone(), ..Finding::default() };
        self.put_link(here, &format!("@{label}"), Linked::Thing { subject });
    }
}

#[cfg(test)]
mod tests {
    use super::lines_of;

    #[test]
    fn lines_of_a_file_are_named_path_colon_range() {
        assert_eq!(lines_of("src/a.rs:10-20"), Some(("src/a.rs".into(), 10, 20)));
        assert_eq!(lines_of("src/a.rs:7"), Some(("src/a.rs".into(), 7, 7)));
        assert_eq!((lines_of("src/a.rs"), lines_of("src/a.rs:9-3"), lines_of(":3")), (None, None, None));
    }
}
