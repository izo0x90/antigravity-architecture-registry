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
        // A tag's link in its colour; a thing's in the accent.
        let colour = match &to {
            Linked::Tag { tag } => self.desk.shared.tags.get().get(tag).map_or(crate::theme::colors().accent, |t| crate::theme::colors().tag(t.colour)),
            Linked::Thing { .. } => crate::theme::colors().accent,
        };
        if self.palette.holding() {
            if let Some(writing) = self.palette.put_back() {
                writing.put_link(label, to, Include::Auto, colour);
            }
            self.input_mode = InputMode::Palette;
            return;
        }
        if let Some(writing) = self.desk.view_mut(here.kind).typing_box() {
            writing.put_link(label, to, Include::Auto, colour);
        }
        self.input_mode = InputMode::Insert { at: here.at };
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
