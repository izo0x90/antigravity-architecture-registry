//! What a message's links pull in: a tag's things (as the tags give them),
//! or one thing (as its owner names it), each to be read by the things
//! layer. A link to what isn't there now (a tag deleted) is left out.

use harness_contracts::{Link, Linked};
use nah_things::{Attached, AttachedThing, Things};

pub(crate) fn attached(tags: &nah_tags::Tags, things: &Things, links: &[Link]) -> Vec<Attached> {
    let mut out: Vec<Attached> = Vec::new();
    for link in links {
        let one = match &link.to {
            Linked::Tag { tag } => tags.attached(tag, link.include),
            Linked::Thing { subject } => {
                let subject = things.current(subject);
                let what = things.name(&subject).unwrap_or_else(|| "something nothing here can show".to_owned());
                Some(Attached { heading: format!("@{what}"), include: link.include, things: vec![AttachedThing { read_by: format!("`thing_read` (\"{what}\")"), what, subject, kept: None, whole: "`thing_read`" }] })
            }
        };
        // The same linked twice: once.
        if let Some(one) = one.filter(|a| !out.iter().any(|o| o.heading == a.heading)) {
            out.push(one);
        }
    }
    out
}
