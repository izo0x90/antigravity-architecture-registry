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

/// The code index's answers, each with the round of asking it was for.
type Answers = std::sync::Arc<std::sync::Mutex<Vec<(u64, crate::net::NetResult<arch_search::Found>)>>>;

/// What the `@` picker found for what was typed, as the server said.
#[derive(Default)]
pub struct Finding {
    /// The query asked about (none: nothing yet), its round (answers to
    /// another are dropped), and how many of its answers came.
    asked: Option<String>,
    round: u64,
    answers: usize,
    found: Vec<Choice>,
    inbox: Answers,
}

/// The server's answers: each box's key, the links checked, what it said.
type Checked = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<harness_contracts::Link>, crate::net::NetResult<Vec<String>>)>>>;

/// What each box's links come to, as the server said: checked a moment
/// after they last changed (what they come to doesn't hang on the text).
#[derive(Default)]
pub struct Checks {
    each: std::collections::BTreeMap<String, Check>,
    inbox: Checked,
}

#[derive(Default)]
struct Check {
    /// The links last seen, when they last changed, whether they're asked.
    links: Vec<harness_contracts::Link>,
    since: f64,
    asked: bool,
    notes: Vec<String>,
    /// Seen this pass (a box gone: let go).
    seen: bool,
}

impl Checks {
    /// Answers taken; every box marked unseen until it's tended.
    fn begin(&mut self, now: f64) {
        for (key, links, said) in std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner)) {
            let Some(check) = self.each.get_mut(&key).filter(|c| c.links == links) else { continue };
            match said {
                Ok(notes) => check.notes = notes,
                // Not checked: asked again a moment from now.
                Err(_) => (check.asked, check.since) = (false, now),
            }
        }
        self.each.values_mut().for_each(|c| c.seen = false);
    }

    /// What box `key`'s `message` comes to (none: nothing to say).
    fn tend(&mut self, key: String, message: &harness_contracts::Message, now: f64, after: f64, net: &crate::net::NetClient, ctx: &egui::Context) -> Option<String> {
        let check = self.each.entry(key.clone()).or_default();
        check.seen = true;
        if check.links != message.links {
            *check = Check { links: message.links.clone(), since: now, seen: true, ..Check::default() };
        }
        if !check.asked {
            if now - check.since >= after {
                check.asked = true;
                let (inbox, links) = (self.inbox.clone(), check.links.clone());
                net.check_links(message, ctx.clone(), move |r| {
                    if let Ok(mut i) = inbox.lock() {
                        i.push((key, links, r));
                    }
                });
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(after));
            }
        }
        (!check.notes.is_empty()).then(|| check.notes.join(" "))
    }

    /// The boxes not tended this pass (gone, or with no links) let go.
    fn end(&mut self) {
        self.each.retain(|_, c| c.seen);
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
    /// Something's being written that takes links: the view's box at
    /// `here`, or the palette's writing (or what it set aside to pick).
    pub(super) fn writing_here(&self, here: Here) -> bool {
        self.desk.view(here.kind).typing().is_some() || self.palette.writes_links() || self.palette.holding()
    }

    /// The box being written in at `here` (the palette's writing, else the view's).
    fn box_here(&mut self, here: Here) -> Option<&mut crate::conversation::TypingBox> {
        if self.palette.writes_links() {
            return self.palette.writing_box();
        }
        self.desk.view_mut(here.kind).typing_box()
    }

    /// What `@` opens: the code's files and symbols (lines with `path:a-b`);
    /// and the link at the caret made whole, a pointer, or as it fits (its
    /// key, or this on a phone).
    pub(super) fn link_actions(&self, here: Here) -> Vec<Action> {
        let picking = self.palette.picking() == Some("link.thing");
        let found = if picking { self.desk.finding.found.clone() } else { Vec::new() };
        let waiting = picking && self.desk.finding.answers < 2 && self.desk.finding.asked.is_some();
        let key = crate::keymap::local().input.link.label();
        let available = |yes: bool| if yes { When::Always } else { When::Unavailable };
        let at_link = self.palette.writing().map_or_else(|| self.desk.view(here.kind).typing().is_some_and(|b| b.link_at_caret()), |b| b.link_at_caret());
        vec![
            Action::new("link.thing", "Link a file or a symbol", format!("Where you're writing: the agent is handed it (whole when small, else its start; {key} at the link changes that)"), ShellCommand::LinkThing)
                .param(Param::Search { hint: "a file or a symbol (path:10-20: those lines)".into(), found, waiting })
                .group(Group::Act)
                .importance(crate::actions::MINOR)
                .when(available(self.writing_here(here))),
            Action::new("link.include", "The link here: whole, a pointer, or as it fits", "In turn: filled goes in whole, underlined goes as its start, plain as it fits", ShellCommand::LinkInclude)
                .group(Group::Act)
                .importance(crate::actions::MINOR)
                .when(available(at_link)),
        ]
    }

    /// The link at the caret where it's written: the next of whole, a
    /// pointer, as it fits.
    pub(super) fn cycle_link(&mut self) {
        let here = self.here();
        if !self.box_here(here).is_some_and(|b| b.cycle_link()) {
            self.note = Some("The caret isn't at a link.".into());
        }
    }

    /// Asks the code index for what the `@` picker's query finds, once per
    /// query; takes the answers.
    pub(super) fn find_links(&mut self, ctx: &egui::Context) {
        let answers: Vec<_> = std::mem::take(&mut *self.desk.finding.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for (round, answer) in answers {
            if round != self.desk.finding.round {
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
        let round = self.desk.finding.round + 1;
        self.desk.finding = Finding { asked: Some(query.clone()), round, inbox: self.desk.finding.inbox.clone(), ..Finding::default() };
        if let Some((path, first, last)) = lines_of(&query) {
            let lines = Subject::CodeLines { path, first, last, snapshot: String::new() };
            let words = lines.name(&harness_contracts::Known::default());
            self.desk.finding.found.push(choice(&lines, words, "lines of a file".into(), egui_phosphor::regular::LIST_NUMBERS));
        }
        if query.trim().is_empty() {
            self.desk.finding.answers = 2;
            return;
        }
        // The code as it is (a plan's components aren't things yet).
        for kind in [arch_search::SearchKind::Files, arch_search::SearchKind::Symbols] {
            let inbox = self.desk.finding.inbox.clone();
            self.desk.net.search(&query, kind, &arch_graph::Source::Code, FOUND_EACH, None, ctx.clone(), move |r| {
                if let Ok(mut i) = inbox.lock() {
                    i.push((round, r));
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

    /// Every box being written in that has links (the views', the
    /// palette's): its links in their colours (a tag's own), and what they
    /// come to checked with the server a moment after they last changed,
    /// said under the box before it's sent.
    pub(super) fn tend_links(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let after = self.config.net.save_after_ms as f64 / 1000.0;
        self.desk.checks.begin(now);
        // Coloured, and what each holds read (boxes with links only).
        // (The tags' colours read only when a tag is linked somewhere.)
        let tags = self.desk.shared.tags.reader();
        let colours: std::cell::OnceCell<std::collections::BTreeMap<harness_contracts::TagId, egui::Color32>> = std::cell::OnceCell::new();
        let colour = |to: &Linked| match to {
            Linked::Tag { tag } => colours.get_or_init(|| tags.get().list().iter().map(|t| (t.id.clone(), crate::theme::colors().tag(t.colour))).collect()).get(tag).copied().unwrap_or(crate::theme::colors().accent),
            Linked::Thing { .. } => crate::theme::colors().accent,
        };
        let mut written: Vec<(String, harness_contracts::Message)> = Vec::new();
        self.desk.each_buffer_by_id(|id, buffer| {
            if let Some(writing) = buffer.typing_box().filter(|b| !b.links().is_empty()) {
                writing.colour_links(&colour);
                written.push((format!("{id:?}"), writing.message()));
            }
        });
        if let Some(writing) = self.palette.writing_box().filter(|b| !b.links().is_empty()) {
            writing.colour_links(&colour);
            written.push(("palette".into(), writing.message()));
        }
        let warnings: std::collections::BTreeMap<String, Option<String>> = written.iter().map(|(key, message)| (key.clone(), self.desk.checks.tend(key.clone(), message, now, after, &self.desk.net, ctx))).collect();
        self.desk.checks.end();
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
        self.desk.finding = Finding { round: self.desk.finding.round, inbox: self.desk.finding.inbox.clone(), ..Finding::default() };
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
