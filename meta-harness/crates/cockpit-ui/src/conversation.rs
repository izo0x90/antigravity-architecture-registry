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

/// Draws `m`: who said it (and what, and when) above, the text, what's under it.
pub fn message(ui: &mut egui::Ui, markdown: &mut CommonMarkCache, m: &Message<'_>) {
    let look = crate::theme::look();
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
            CommonMarkViewer::new().show(ui, markdown, m.text);
        } else {
            let text = RichText::new(m.text).size(look.text_body).color(colors().text_strong);
            ui.label(if m.by == harness_contracts::USER { text.strong() } else { text });
        }
    }
    for line in &m.under {
        ui.label(RichText::new(line).size(look.text_small).monospace().color(colors().muted));
    }
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

/// The typing box: what's written in it, kept between frames.
#[derive(Default)]
pub struct TypingBox {
    pub text: String,
    /// Text was put in (not typed): the caret goes after it.
    caret_to_end: bool,
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

    /// What's written, trimmed, taken out of the box.
    pub fn take(&mut self) -> String {
        std::mem::take(&mut self.text).trim().to_owned()
    }

    /// Draws it (`rows` high, `hint` while empty); it has the keyboard while
    /// `typing` (its host says). Once sent, the keyboard is given back (the
    /// next key is a command's).
    pub fn draw(&mut self, ui: &mut egui::Ui, id: egui::Id, rows: usize, hint: &str, typing: bool) -> Typed {
        let keys = crate::keymap::local();
        let before = self.text.chars().count();
        // Focused before it's drawn: the first key of the pass is its (the
        // key that opened it ran a command, and isn't typed: `typed_away`).
        if typing {
            ui.memory_mut(|m| m.request_focus(id));
        }
        let output = ui.add(keys.text_box(&mut self.text).id(id).desired_rows(rows).desired_width(f32::INFINITY).hint_text(hint));
        if std::mem::take(&mut self.caret_to_end) {
            caret_to_end(ui.ctx(), output.id, &self.text);
        }
        let sent = if typing && output.has_focus() { ui.input(|i| keys.input.sent(i)) } else { None };
        if sent.is_some() {
            output.surrender_focus();
        }
        // Typed at its end (what's picked is put there): a trigger typed amid
        // what's written is just a character.
        let at_end = egui::TextEdit::load_state(ui.ctx(), output.id).and_then(|s| s.cursor.char_range()).is_none_or(|r| r.primary.index == self.text.chars().count());
        let trigger = (output.changed() && at_end).then(|| crate::trigger::typed(&self.text, before)).flatten();
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

    #[test]
    fn what_is_put_after_a_trigger_doesnt_double_it() {
        let mut b = TypingBox { text: "see #".into(), ..TypingBox::default() };
        b.put("#flaky ");
        assert_eq!(b.text, "see #flaky ", "the # typed isn't doubled");
        assert_eq!(b.take(), "see #flaky");
        assert!(b.text.is_empty());
    }

    #[test]
    fn who_wrote_something_is_said_one_way() {
        assert_eq!((who("user"), who("lead"), who("")), ("you", "lead", "someone"));
    }
}
