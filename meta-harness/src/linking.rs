//! What a message's links pull in: a tag's things (as the tags give them),
//! or one thing (as its owner names it), each to be read by the things
//! layer. A link is headed by what the message says at it (its span), so
//! the reader can tie the two; a tag deleted since it was linked is said.

use harness_contracts::{Include, Link, Linked};
use nah_things::{Attached, AttachedThing, Things};

/// What `text` says at `link` (its span; none: a span that isn't in it).
fn said_at(text: &str, link: &Link) -> Option<String> {
    (link.start < link.end)
        .then(|| text.chars().skip(link.start).take(link.end - link.start).collect::<String>())
        .filter(|s| s.chars().count() == link.end - link.start)
}

/// A part for each tag or thing `links` link (in `text`), with every link
/// to it; linked twice, it's whole if either link asks for it.
pub(crate) fn attached(tags: &nah_tags::Tags, things: &Things, text: &str, links: &[Link]) -> Vec<(Vec<Link>, Attached)> {
    let mut out: Vec<(Vec<Link>, Attached)> = Vec::new();
    things.pass(|| {
        for link in links {
            if let Some((to, part)) = out.iter_mut().find(|(to, _)| to[0].to == link.to) {
                if link.include == Include::Whole {
                    part.include = Include::Whole;
                }
                to.push(link.clone());
                continue;
            }
            let said = said_at(text, link);
            let part = match &link.to {
                Linked::Tag { tag } => match tags.attached(tag, link.include) {
                    Some(attached) => attached,
                    // Deleted since it was linked: said, by what the message says there.
                    None => {
                        let heading = said.clone().unwrap_or_else(|| tag.to_string());
                        Attached { heading, include: link.include, things: Vec::new(), gone: true }
                    }
                },
                Linked::Thing { subject } => {
                    let subject = things.current(subject);
                    let what = things.name(&subject).unwrap_or_else(|| "something nothing here can show".to_owned());
                    let heading = match said {
                        Some(said) if said.trim_start_matches('@') != what => format!("{said} ({what})"),
                        _ => format!("@{what}"),
                    };
                    Attached {
                        heading,
                        include: link.include,
                        things: vec![AttachedThing {
                            read_by: format!("`thing_read` (\"{what}\")"),
                            what,
                            subject,
                            kept: None,
                            whole: "`thing_read`",
                        }],
                        gone: false,
                    }
                }
            };
            out.push((vec![link.clone()], part));
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_links_span_is_what_the_text_says_there_and_a_bad_one_is_nothing() {
        let link = |start, end| Link { start, end, to: Linked::Tag { tag: harness_contracts::TagId::new("x") }, include: Include::Auto };
        assert_eq!(said_at("see @a.rs now", &link(4, 9)).as_deref(), Some("@a.rs"));
        assert_eq!((said_at("see", &link(2, 9)), said_at("see", &link(2, 1))), (None, None));
    }
}
