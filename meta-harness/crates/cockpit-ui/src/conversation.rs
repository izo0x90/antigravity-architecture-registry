//! The one conversation component: a message as every conversation draws
//! one (who, when, what it is, the text, what's under it), and the typing
//! box (many lines; Enter sends, as the keymap says; a `#` typed where a
//! word starts asks for a tag's name, which its host gets the shell to
//! pick). The chat draws your messages and the agent's with it; a
//! discussion draws everything said in it; a note is written in its box.

use egui::{Color32, Rect, RichText};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::theme::colors;

/// Who wrote something, in words: you, or the agent's slot (one way for
/// every place that says who: messages, tags, notes).
pub fn who(by: &str) -> &str {
    harness_contracts::by_to_user(by)
}

/// A time a message was said, in words (`2026-10-20 22:11`, UTC).
pub fn when(at_ms: u64) -> String {
    let written = harness_contracts::when::written(at_ms);
    written.get(..16).map_or(written.clone(), |minutes| minutes.replace('T', " "))
}

/// One message, as a conversation draws it.
pub struct Message<'a> {
    /// Who said it (`user`, or an agent's slot).
    pub by: &'a str,
    /// When (milliseconds since 1970), when it's known.
    pub at_ms: Option<u64>,
    /// What it is, when it isn't a plain message ("answered", "settled it").
    pub what: Option<&'a str>,
    pub text: &'a str,
    /// Written as markdown (an agent's), else as it is.
    pub markdown: bool,
    /// Its colour, when it says something has happened (an answer, a
    /// settling, a reopening).
    pub tone: Option<Color32>,
    /// Lines under it (what an answer changed).
    pub under: Vec<String>,
}

impl<'a> Message<'a> {
    pub fn new(by: &'a str, text: &'a str) -> Self {
        Self { by, at_ms: None, what: None, text, markdown: false, tone: None, under: Vec::new() }
    }
}

/// What a view draws messages with: the markdown's cache, and code
/// blocks' colours (the desk's, read).
#[derive(Default)]
pub struct Messages {
    markdown: CommonMarkCache,
    code: crate::shared::Reader<crate::highlighter::Highlighter>,
}

impl Messages {
    /// The desk's code colours, read from now on.
    pub fn follow(&mut self, shared: &crate::shared::Shared) {
        self.code.follow(&shared.highlighter);
    }

    /// Draws `m` (see `message`): where its code blocks were drawn.
    pub fn draw(&mut self, ui: &mut egui::Ui, m: &Message<'_>) -> Vec<Rect> {
        message(ui, &mut self.markdown, &self.code.get(), m)
    }

    /// Draws markdown `text`, its parts as a message's are: where its code
    /// blocks were drawn.
    pub fn markdown(&mut self, ui: &mut egui::Ui, text: &str) -> Vec<Rect> {
        markdown(ui, &mut self.markdown, &self.code.get(), text)
    }
}

/// Draws `m`: who said it (and what, and when) above, the text, what's
/// under it. Its code blocks coloured as `code` has them; where they were
/// drawn, in order.
fn message(ui: &mut egui::Ui, cache: &mut CommonMarkCache, code: &crate::highlighter::Highlighter, m: &Message<'_>) -> Vec<Rect> {
    let look = crate::theme::look();
    let mut drawn = Vec::new();
    let mut head = who(m.by).to_owned();
    if let Some(what) = m.what {
        head = format!("{head} {what}");
    }
    if let Some(at) = m.at_ms {
        head = format!("{head} · {}", when(at));
    }
    ui.label(RichText::new(head).size(look.text_small).color(m.tone.unwrap_or(colors().muted)));
    if !m.text.is_empty() {
        if m.markdown {
            drawn = markdown(ui, cache, code, m.text);
        } else {
            let text = RichText::new(m.text).size(look.text_body).color(colors().text_strong);
            ui.label(if m.by == harness_contracts::USER { text.strong() } else { text });
        }
    }
    for line in &m.under {
        ui.label(RichText::new(line).size(look.text_small).monospace().color(colors().muted));
    }
    drawn
}

/// Markdown `text`, part by part. Its words wrap in the width it's given;
/// a table's rows and a code block's lines can't, so each scrolls
/// sideways in a box of that width, never widening what holds it.
fn markdown(ui: &mut egui::Ui, cache: &mut CommonMarkCache, code: &crate::highlighter::Highlighter, text: &str) -> Vec<Rect> {
    let mut drawn = Vec::new();
    for part in parts(text) {
        match part {
            Part::Words(words) => {
                CommonMarkViewer::new().show(ui, cache, words);
            }
            Part::Table(table) => {
                egui::ScrollArea::horizontal().id_salt(ui.next_auto_id()).auto_shrink([false, true]).show(ui, |ui| {
                    CommonMarkViewer::new().show(ui, cache, table);
                });
            }
            Part::Code { fence, code: text } => drawn.push(code_block(ui, code, fence, text)),
        }
    }
    drawn
}

/// A code block: its text in the code's face, coloured as the language its
/// fence names (as the code view colours a file), in a box of the width
/// it's given. Where it was drawn.
fn code_block(ui: &mut egui::Ui, code: &crate::highlighter::Highlighter, fence: &str, text: &str) -> Rect {
    let look = crate::theme::look();
    let colours = code.colours(fence, text);
    let font = egui::FontId::monospace(look.text_body);
    let format = |color: Color32| egui::text::TextFormat { font_id: font.clone(), color, ..Default::default() };
    let job = crate::highlighter::coloured(text, 0..text.len(), colours.as_deref(), |capture| code.colour(capture), colors().text, format);
    egui::Frame::NONE
        .fill(colors().surface)
        .corner_radius(look.radius_s)
        .inner_margin(crate::theme::margin(look.space_sm))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::horizontal().id_salt(ui.next_auto_id()).auto_shrink([false, true]).show(ui, |ui| {
                ui.add(egui::Label::new(job).extend());
            });
        })
        .response
        .rect
}

/// A part of a message's markdown: words, a table, a code block (the
/// language its fence names, its text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part<'a> {
    Words(&'a str),
    Table(&'a str),
    Code { fence: &'a str, code: &'a str },
}

/// Markdown cut into its parts, in order: its fenced code blocks (three
/// or more backticks or tildes; one not closed yet runs to the end, as one
/// being written does), and around them its tables and words.
pub fn parts(text: &str) -> Vec<Part<'_>> {
    let mut out = Vec::new();
    // (Where the words before the next block start; where a line starts.)
    let (mut words, mut at) = (0, 0);
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        let start = at;
        at += line.len();
        let Some((mark, fence)) = opening(line) else { continue };
        out.extend(tables_apart(&text[words..start]));
        // Its lines, to the one closing it (or the end).
        let (body, mut end) = (at, text.len());
        for line in lines.by_ref() {
            let line_start = at;
            at += line.len();
            if closes(line, mark) {
                end = line_start;
                break;
            }
        }
        out.push(Part::Code { fence, code: text[body..end].trim_end_matches(['\n', '\r']) });
        words = at;
    }
    out.extend(tables_apart(&text[words..]));
    out
}

/// A fence opening a code block: its mark (backticks or tildes, as many as
/// are written) and the language it names.
fn opening(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let mark_char = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let mark_len = trimmed.chars().take_while(|c| *c == mark_char).count();
    if mark_len < 3 {
        return None;
    }
    let (mark, info) = trimmed.split_at(mark_len);
    // (A backtick fence's words can't hold a backtick.)
    if mark_char == '`' && info.contains('`') {
        return None;
    }
    Some((mark, info.split_whitespace().next().unwrap_or("")))
}

/// Whether `line` closes a block opened with `mark`: the same character,
/// at least as many, nothing after, at most three spaces in.
fn closes(line: &str, mark: &str) -> bool {
    let trimmed = line.trim();
    let c = mark.as_bytes()[0] as char;
    trimmed.len() >= mark.len() && trimmed.chars().all(|x| x == c) && line.len() - line.trim_start_matches(' ').len() <= 3
}

/// Markdown cut around its tables (lines of cells, `| … |`, with a
/// `|---|` line among them), in order.
fn tables_apart(text: &str) -> Vec<Part<'_>> {
    // Runs of lines that are rows of cells, or aren't, by where they are.
    let mut runs: Vec<(usize, usize, bool)> = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let row = line.trim_start().starts_with('|');
        match runs.last_mut() {
            Some((_, end, rows)) if *rows == row => *end = at + line.len(),
            _ => runs.push((at, at + line.len(), row)),
        }
        at += line.len();
    }
    // Rows are a table only with their `|---|` line; else they're text,
    // joined with the text around them.
    let mut parts: Vec<(usize, usize, bool)> = Vec::new();
    for (from, to, rows) in runs {
        let table = rows && text[from..to].lines().any(|l| l.contains("--"));
        match parts.last_mut() {
            Some((_, end, false)) if !table => *end = to,
            _ => parts.push((from, to, table)),
        }
    }
    parts.into_iter().filter(|(from, to, _)| !text[*from..*to].trim().is_empty()).map(|(from, to, table)| if table { Part::Table(&text[from..to]) } else { Part::Words(&text[from..to]) }).collect()
}

/// What's said in a discussion, as a message: an answer (what it changed
/// under it), a settling, a reopening said so.
pub fn said<'a>(d: &'a harness_contracts::Discussion, s: &'a harness_contracts::Said) -> Message<'a> {
    use harness_contracts::SaidKind;
    let tone = match s.kind {
        SaidKind::Message => None,
        SaidKind::Answer => Some(colors().info),
        SaidKind::Settled => Some(colors().ok),
        SaidKind::Reopened => Some(colors().warn),
    };
    let what = (s.kind != SaidKind::Message).then(|| s.kind.words());
    Message { by: d.name_of(&s.by), at_ms: Some(s.at_ms), what, text: &s.text, markdown: s.by != harness_contracts::USER, tone, under: s.changed.iter().map(|c| format!("changed {}", c.what)).collect() }
}

/// Whose move it is in a discussion, in a few words ("waiting on you",
/// "waiting on lead", "settled"), and whether it's yours.
pub fn whose(d: &harness_contracts::Discussion) -> (String, bool) {
    match d.whose_move() {
        harness_contracts::Move::User => ("waiting on you".to_owned(), true),
        harness_contracts::Move::Agent(slot) => (format!("waiting on {}", d.name_of(&slot)), false),
        harness_contracts::Move::Nobody => ("settled".to_owned(), false),
    }
}

/// Where a discussion stands, in a word, and its colour.
pub fn standing(d: &harness_contracts::Discussion) -> (&'static str, Color32) {
    match d.status {
        harness_contracts::DiscussionStatus::Open => ("OPEN", colors().warn),
        harness_contracts::DiscussionStatus::Answered => ("ANSWERED", colors().info),
        harness_contracts::DiscussionStatus::Settled => ("SETTLED", colors().ok),
    }
}

/// A discussion's line above what's said: where it stands, what it is,
/// its id, who's asked.
pub fn heading(ui: &mut egui::Ui, d: &harness_contracts::Discussion) {
    let look = crate::theme::look();
    let (word, colour) = standing(d);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(word).size(look.text_small).strong().color(colour));
        if d.kind == harness_contracts::DiscussionKind::ChangeRequest {
            ui.label(RichText::new("change request").size(look.text_small).color(colors().special));
        }
        ui.label(RichText::new(format!("{} · asks {}", d.id, d.to_user(&d.asked))).size(look.text_small).color(colors().muted));
    });
}

/// The typing box: what's written in it, kept between frames, and what
/// it links (spans of it that pull a tag or a thing into the message):
/// drawn as chips, kept where they are as the text is edited around them,
/// dropped when the text in one is.
#[derive(Default)]
pub struct TypingBox {
    pub text: String,
    links: Vec<Chip>,
    /// Where the caret was last drawn (characters from the start).
    caret: Option<usize>,
    /// Said under it (what its links come to, before it's sent).
    pub warning: Option<String>,
    /// Text was put in (not typed): the caret goes after it.
    caret_to_end: bool,
}

/// A link in the box, and the colour it's drawn in.
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub link: harness_contracts::Link,
    pub colour: Color32,
}

/// What came of a frame of the typing box.
pub struct Typed {
    /// Where it is (while it's typed in, keys go there).
    pub rect: Rect,
    /// Sent (Enter; adding to a running turn, the chat's other way).
    pub sent: Option<crate::keymap::Sent>,
    /// Clicked into: its host types in it.
    pub clicked: bool,
    /// A trigger character just typed where a word starts (`crate::trigger`):
    /// its picker, for what follows it.
    pub trigger: Option<char>,
}

impl TypingBox {
    /// `text` put after what's written (a trigger it starts with that the
    /// box ends with isn't doubled), the caret after it.
    pub fn put(&mut self, text: &str) {
        let text = match text.chars().next() {
            Some(c) if crate::trigger::is_trigger(c) && self.text.ends_with(c) => &text[c.len_utf8()..],
            _ => text,
        };
        self.text.push_str(text);
        self.caret_to_end = true;
    }

    /// `label` (`#flaky`, `@src/a.rs`) put after what's written as a link
    /// to `to` (a trigger it starts with that the box ends with is the
    /// link's first character), then a space; the caret after it.
    pub fn put_link(&mut self, label: &str, to: harness_contracts::Linked, include: harness_contracts::Include) {
        let doubled = label.chars().next().is_some_and(|c| crate::trigger::is_trigger(c) && self.text.ends_with(c));
        let start = self.text.chars().count() - usize::from(doubled);
        self.put(label);
        let end = self.text.chars().count();
        self.text.push(' ');
        self.links.push(Chip { link: harness_contracts::Link { start, end, to, include }, colour: colors().accent });
    }

    /// Emptied: its text and its links.
    pub fn clear(&mut self) {
        (self.text, self.links) = (String::new(), Vec::new());
    }

    /// Whether it holds `draft` as it is (its text and links).
    pub fn holds(&self, draft: &harness_contracts::Message) -> bool {
        self.text == draft.text && self.links.len() == draft.links.len() && self.links.iter().zip(&draft.links).all(|(c, l)| c.link == *l)
    }

    /// A link the caret is in or just after (where it is once put in).
    pub fn link_at_caret(&self) -> bool {
        self.caret.is_some_and(|at| self.links.iter().any(|c| c.link.start < at && at <= c.link.end + 1))
    }

    /// The links in it, in order.
    pub fn links(&self) -> &[Chip] {
        &self.links
    }

    /// Each link drawn in the colour `colour` gives it (the shell's: a
    /// tag's own).
    pub fn colour_links(&mut self, colour: &dyn Fn(&harness_contracts::Linked) -> Color32) {
        for chip in &mut self.links {
            chip.colour = colour(&chip.link.to);
        }
    }

    /// The link the caret is in or just after: whole, a pointer, or as it
    /// fits, in turn. False: there's none there.
    pub fn cycle_link(&mut self) -> bool {
        let Some(at) = self.caret else { return false };
        // In it, or just after it (where the caret is once it's put in).
        match self.links.iter_mut().find(|c| c.link.start < at && at <= c.link.end + 1) {
            Some(chip) => {
                chip.link.include = chip.link.include.next();
                true
            }
            None => false,
        }
    }

    /// What's written, trimmed, and what it links, taken out of the box.
    pub fn take(&mut self) -> harness_contracts::Message {
        std::mem::take(self).message()
    }

    /// What's written as it is (untrimmed), and what it links: what's kept
    /// of it unsent.
    pub fn draft(&self) -> harness_contracts::Message {
        harness_contracts::Message { text: self.text.clone(), links: self.links.iter().map(|c| c.link.clone()).collect() }
    }

    /// What's written, trimmed, and what it links.
    pub fn message(&self) -> harness_contracts::Message {
        harness_contracts::Message { text: self.text.clone(), links: self.links.iter().map(|c| c.link.clone()).collect() }.trimmed()
    }

    /// A box taken out before (a send that failed) put back as it was.
    pub fn put_back(&mut self, kept: TypingBox) {
        *self = TypingBox { caret_to_end: true, ..kept };
    }

    /// `message` written in it again (a draft kept, a start to write on
    /// from), its links as they were (coloured by the shell).
    pub fn restore(&mut self, message: &harness_contracts::Message) {
        self.text.clone_from(&message.text);
        self.links = message.links.iter().map(|l| Chip { link: l.clone(), colour: colors().accent }).collect();
        self.caret_to_end = true;
    }

    /// The links kept where they are through an edit (`before` was the
    /// text): one the edit reached into is dropped.
    fn follow_edit(&mut self, before: &str, caret: Option<usize>) {
        if self.links.is_empty() || before == self.text {
            return;
        }
        let (old, new): (Vec<char>, Vec<char>) = (before.chars().collect(), self.text.chars().collect());
        // Where the edit began: what's the same at the start, unless the
        // caret (after it) says it began earlier (a character typed that
        // repeats the one after it looks typed later than it was).
        let grew = new.len().saturating_sub(old.len());
        let same_start = old.iter().zip(&new).take_while(|(a, b)| a == b).count().min(caret.map_or(usize::MAX, |c| c.saturating_sub(grew)));
        let same_end = old[same_start..].iter().rev().zip(new[same_start..].iter().rev()).take_while(|(a, b)| a == b).count();
        let (old_end, moved) = (old.len() - same_end, new.len() as isize - old.len() as isize);
        self.links.retain_mut(|c| {
            if c.link.end <= same_start {
                true
            } else if c.link.start >= old_end {
                c.link.start = c.link.start.saturating_add_signed(moved);
                c.link.end = c.link.end.saturating_add_signed(moved);
                true
            } else {
                false
            }
        });
    }

    /// The text laid out with its links as chips: as it fits in the
    /// link's colour, whole on a fill of it, a pointer underlined.
    fn laid_out(&self, ui: &egui::Ui, text: &str, wrap: f32) -> std::sync::Arc<egui::Galley> {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let plain = egui::TextFormat::simple(font.clone(), ui.visuals().text_color());
        let mut job = egui::text::LayoutJob { wrap: egui::text::TextWrapping { max_width: wrap, ..Default::default() }, ..Default::default() };
        // Every character's byte offset, once (the chips' spans are in characters).
        let bytes: Vec<usize> = text.char_indices().map(|(b, _)| b).chain(std::iter::once(text.len())).collect();
        let mut at = 0;
        let mut chips: Vec<&Chip> = self.links.iter().filter(|c| c.link.start < c.link.end && c.link.end < bytes.len()).collect();
        chips.sort_by_key(|c| c.link.start);
        for chip in chips {
            let (start, end) = (bytes[chip.link.start], bytes[chip.link.end]);
            if start < at {
                continue;
            }
            job.append(&text[at..start], 0.0, plain.clone());
            let mut format = egui::TextFormat::simple(font.clone(), chip.colour);
            match chip.link.include {
                harness_contracts::Include::Auto => {}
                harness_contracts::Include::Whole => format.background = chip.colour.gamma_multiply(0.25),
                harness_contracts::Include::Pointer => format.underline = egui::Stroke::new(crate::theme::look().line_thin, chip.colour),
            }
            job.append(&text[start..end], 0.0, format);
            at = end;
        }
        job.append(&text[at..], 0.0, plain);
        ui.fonts_mut(|f| f.layout_job(job))
    }

    /// Draws it (`rows` high, `hint` while empty); it has the keyboard while
    /// `typing` (its host says). Once sent, the keyboard is given back (the
    /// next key is a command's).
    pub fn draw(&mut self, ui: &mut egui::Ui, id: egui::Id, rows: usize, hint: &str, typing: bool) -> Typed {
        // The box, and what's said under it, one under the other wherever
        // it's laid out (a row of the chat's, say).
        ui.vertical(|ui| self.draw_in(ui, id, rows, hint, typing)).inner
    }

    fn draw_in(&mut self, ui: &mut egui::Ui, id: egui::Id, rows: usize, hint: &str, typing: bool) -> Typed {
        let keys = crate::keymap::local();
        let before = self.text.chars().count();
        // Focused before it's drawn: the first key of the pass is its (the
        // key that opened it ran a command, and isn't typed: `typed_away`).
        if typing {
            ui.memory_mut(|m| m.request_focus(id));
        }
        // The link at the caret, cycled by its key (before the box takes the key).
        if typing && ui.memory(|m| m.has_focus(id)) && ui.input_mut(|i| keys.input.link.consume(i)) {
            self.cycle_link();
        }
        // Text put in: the caret after it before this frame's keys are typed
        // (they follow it).
        if std::mem::take(&mut self.caret_to_end) {
            caret_to_end(ui.ctx(), id, &self.text);
        }
        // (What it held, to follow its links through the edit: only when it has some.)
        let written = (!self.links.is_empty()).then(|| self.text.clone());
        let chips = TypingBox { links: self.links.clone(), ..TypingBox::default() };
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| chips.laid_out(ui, text.as_str(), wrap);
        let output = ui.add(keys.text_box(&mut self.text).id(id).desired_rows(rows).desired_width(f32::INFINITY).hint_text(hint).layouter(&mut layouter));
        let caret_now = egui::TextEdit::load_state(ui.ctx(), output.id).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index);
        if let Some(written) = written {
            self.follow_edit(&written, caret_now);
        }
        self.caret = caret_now;
        let sent = if typing && output.has_focus() { ui.input(|i| keys.input.sent(i)) } else { None };
        if sent.is_some() {
            output.surrender_focus();
        }
        // Typed at its end (what's picked is put there): a trigger typed amid
        // what's written is just a character.
        let at_end = egui::TextEdit::load_state(ui.ctx(), output.id).and_then(|s| s.cursor.char_range()).is_none_or(|r| r.primary.index == self.text.chars().count());
        let trigger = (output.changed() && at_end).then(|| crate::trigger::typed(&self.text, before)).flatten();
        // What its links come to, said before it's sent.
        if let Some(warning) = &self.warning {
            ui.label(RichText::new(warning).size(crate::theme::look().text_small).color(colors().warn));
        }
        Typed { rect: output.rect, sent, clicked: output.clicked() || output.gained_focus(), trigger }
    }
}

/// Puts a text field's caret after its text (text put in it, not typed).
pub fn caret_to_end(ctx: &egui::Context, id: egui::Id, text: &str) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let end = egui::text::CCursor::new(text.chars().count());
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
    state.store(ctx, id);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message's markdown cut around its tables: text, a table (its rows
    /// with a `|---|` line), text; rows without that line stay text.
    #[test]
    fn tables_are_cut_apart_from_the_words_around_them() {
        let text = "Here:\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nand after.\n| not | a table |\n";
        let cut = parts(text);
        assert!(matches!(cut[..], [Part::Words(_), Part::Table("| a | b |\n|---|---|\n| 1 | 2 |\n"), Part::Words(after)] if after.contains("and after") && after.contains("| not | a table |")), "{cut:?}");
        let whole: String = cut.iter().map(|p| match p { Part::Words(w) | Part::Table(w) => *w, Part::Code { .. } => "" }).collect();
        assert_eq!(whole, text, "nothing lost");
        assert_eq!(parts("just words"), [Part::Words("just words")]);
    }

    /// Fenced code blocks are parts of their own (the language their fence
    /// names, their lines without the fences), with the words and tables
    /// around them; a fence of tildes, or longer than three, closes only
    /// with its own; one not closed runs to the end; a table in a block is
    /// code.
    #[test]
    fn code_blocks_are_cut_apart_with_their_language() {
        let text = "Try:\n```rust\nfn main() {}\n```\nThen:\n~~~~ py extra\nprint(1)\n```\n~~~~\n| a |\n|---|\n";
        assert_eq!(parts(text), [
            Part::Words("Try:\n"),
            Part::Code { fence: "rust", code: "fn main() {}" },
            Part::Words("Then:\n"),
            Part::Code { fence: "py", code: "print(1)\n```" },
            Part::Table("| a |\n|---|\n"),
        ]);
        assert_eq!(parts("```\n| a |\n|---|\nstill"), [Part::Code { fence: "", code: "| a |\n|---|\nstill" }], "not closed yet: to the end");
        assert_eq!(parts("    ```\nindented: words"), [Part::Words("    ```\nindented: words")], "four spaces in: no fence");
    }

    #[test]
    fn what_is_put_after_a_trigger_doesnt_double_it() {
        let mut b = TypingBox { text: "see #".into(), ..TypingBox::default() };
        b.put("#flaky ");
        assert_eq!(b.text, "see #flaky ", "the # typed isn't doubled");
        assert_eq!(b.take().text, "see #flaky");
        assert!(b.text.is_empty());
    }

    /// A link is a span of what's written: put after a trigger (which it
    /// starts with), kept where it is as the text is edited around it,
    /// dropped when the text in it is, cycled at the caret, taken with
    /// the text (trimmed, moved with it).
    #[test]
    fn a_link_is_kept_through_edits_around_it_and_dropped_by_one_in_it() {
        use harness_contracts::{Include, Linked, TagId};
        let tag = |t: &str| Linked::Tag { tag: TagId::new(t) };
        let mut b = TypingBox { text: "  see #".into(), ..TypingBox::default() };
        b.put_link("#flaky", tag("x1"), Include::Auto);
        assert_eq!(b.text, "  see #flaky ");
        assert_eq!((b.links()[0].link.start, b.links()[0].link.end), (6, 12), "the # typed is its first character");
        // Typed before it: it moves; after it: it stays.
        let before = b.text.clone();
        b.text.insert_str(2, "go ");
        b.follow_edit(&before, None);
        assert_eq!((b.links()[0].link.start, b.links()[0].link.end), (9, 15));
        let before = b.text.clone();
        b.text.push_str("now");
        b.follow_edit(&before, None);
        assert_eq!(b.links().len(), 1);
        // The caret in it (or just after it): cycled.
        b.caret = Some(16);
        assert!(b.cycle_link() && b.links()[0].link.include == Include::Whole);
        b.caret = Some(3);
        assert!(!b.cycle_link(), "not in a link");
        // Taken: trimmed, the link moved with the text.
        let mut taken = TypingBox { text: b.text.clone(), links: b.links.clone(), ..TypingBox::default() };
        let message = taken.take();
        assert_eq!((message.text.as_str(), message.links[0].start, message.links[0].end), ("go see #flaky now", 7, 13));
        assert!(taken.links().is_empty());
        // Typed inside it, where the character repeats the one after: the
        // caret says it's in it, so it goes.
        let mut c = TypingBox::default();
        c.put_link("#flaky", tag("x1"), Include::Auto);
        let before = c.text.clone();
        c.text.insert(5, 'y');
        c.follow_edit(&before, Some(6));
        assert!(c.links().is_empty(), "typed in it: {:?}", c.links());
        // A # typed just before it: outside it, it moves on.
        let mut d = TypingBox::default();
        d.put_link("#flaky", tag("x1"), Include::Auto);
        let before = d.text.clone();
        d.text.insert(0, '#');
        d.follow_edit(&before, Some(1));
        assert_eq!((d.links()[0].link.start, d.links()[0].link.end), (1, 7));
        // A character taken out of it: the link goes (the text stays text).
        let before = b.text.clone();
        b.text.remove(12);
        b.follow_edit(&before, None);
        assert!(b.links().is_empty());
    }

    #[test]
    fn who_wrote_something_is_said_one_way() {
        assert_eq!((who("user"), who("lead"), who("")), ("you", "lead", "someone"));
    }
}
