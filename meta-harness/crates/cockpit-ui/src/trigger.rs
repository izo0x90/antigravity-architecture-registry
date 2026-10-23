//! Trigger characters: typed where a word starts in any typing box, each
//! opens an action's picker for what follows it (`#`: a tag's name). The
//! box reports the character; the shell opens the action it maps to with
//! the box's text set aside; what's picked is put in as a link (`#`: a tag;
//! `@`: a file or a symbol).
//! The table is the only place that says which character means what.

/// Each trigger character and the action its picker is.
pub const TRIGGERS: &[(char, &str)] = &[('#', "tag.insert"), ('@', "link.thing")];

/// The action `c` opens, when it's a trigger.
pub fn action_for(c: char) -> Option<&'static str> {
    TRIGGERS.iter().find(|(t, _)| *t == c).map(|(_, id)| *id)
}

pub fn is_trigger(c: char) -> bool {
    action_for(c).is_some()
}

/// Whether `text` (`before` characters long a moment ago) just had a
/// trigger typed where a word starts: that character. After Backspace
/// too, one typed is one typed; pasted, it isn't.
pub fn typed(text: &str, before: usize) -> Option<char> {
    let last = text.chars().last().filter(|c| is_trigger(*c))?;
    let one_typed = text.chars().count() == before + 1;
    let starts_word = text.chars().rev().nth(1).is_none_or(|c| c.is_whitespace() || "([{\"'".contains(c));
    (one_typed && starts_word).then_some(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trigger_typed_where_a_word_starts_is_reported() {
        assert_eq!((typed("#", 0), typed("see #", 4), typed("(#", 1)), (Some('#'), Some('#'), Some('#')));
        assert_eq!(typed("a#", 1), None, "inside a word: a character");
        assert_eq!(typed("see #", 3), None, "pasted, not typed");
        assert_eq!(typed("see x", 4), None, "not a trigger");
        assert_eq!(action_for('#'), Some("tag.insert"));
    }
}
