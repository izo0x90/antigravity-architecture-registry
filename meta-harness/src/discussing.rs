//! Discussions and the agents' slots: who a discussion asks (the lead
//! unless another slot is named, or the user when an agent asks them), and
//! the agents in it told what others say in it: the slot asked, and the
//! slot that started it (as a sub-slot's report is: in its running
//! session, else held until one runs).

use std::collections::HashMap;
use std::sync::Arc;

use harness_contracts::{ClientEvent, Discussion, DiscussionId, DiscussionKind, SaidKind};
use harness_core::{EventHub, SlotManager};
use nah_discussions::{Discussions, USER};

/// Who's asked, from what was named (a slot's id or name; none: the lead;
/// `user`: the user, asked by an agent): a slot's id, or `user`.
pub fn asked(slots: &SlotManager, named: Option<&str>, by: &str) -> Result<String, String> {
    let named = named.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(nah_discussions::LEAD);
    if named == USER {
        return if by == USER { Err("you can't ask yourself: ask an agent slot (or none: the lead)".into()) } else { Ok(USER.to_owned()) };
    }
    let slot = slots
        .named(named)
        .ok_or_else(|| format!("there's no agent slot {named:?} to ask, or several are named so (name it by its id; none: the lead)"))?;
    if slot.id.as_str() == by {
        return Err("you can't ask yourself: ask the user, or another slot".into());
    }
    Ok(slot.id.to_string())
}

/// Whether the one asked (`user`, or a slot's id) may read `subject` (a
/// slot asked about another's chat could never answer).
pub(crate) fn may_read(
    slots: &Arc<SlotManager>,
    access: &Arc<nah_access::Access>,
    discussions: &Arc<Discussions>,
    asked: &str,
    subject: &harness_contracts::Subject,
) -> bool {
    if asked == USER {
        return true;
    }
    match slots.get(&harness_contracts::SlotId::new(asked)) {
        Some(slot) => crate::mcp::sets::reach::Reach::of_slot(slots, access, discussions, &slot).reaches(subject),
        None => false,
    }
}

/// What an agent in it is told of what was said (`at`: which of its
/// messages; `asked`: the agent is the one asked, else the one that
/// started it).
fn news(d: &Discussion, at: usize, asked: bool) -> Option<harness_contracts::Message> {
    let said = d.said.get(at)?;
    let what = match (at, said.kind, d.kind, asked) {
        (0, _, DiscussionKind::ChangeRequest, true) => "asks you for a change",
        (0, _, DiscussionKind::Question, true) => "asks you",
        (0, ..) => return None,
        (_, SaidKind::Reopened, _, _) => "reopened it",
        (_, SaidKind::Message, _, _) => "said",
        (_, SaidKind::Answer, _, false) => "answered it",
        (_, SaidKind::Settled, _, false) => "settled it",
        _ => return None,
    };
    let then = match (asked, d.kind) {
        (true, DiscussionKind::ChangeRequest) => "discussion_answer it, naming what you changed, or discussion_reply to ask something back",
        (true, DiscussionKind::Question) => "discussion_answer it, or discussion_reply to ask something back",
        (false, _) => "discussion_reply says something in it",
    };
    // What the user's message links goes with it (an agent's is plain
    // text), its links moved with its text.
    let links = if said.by == USER { said.links.clone() } else { Vec::new() };
    let mut news = harness_contracts::Message { text: said.text.clone(), links }.after(&format!(
        "Discussion {} on {}: {} {what}:\n\n",
        d.id,
        d.on.what,
        d.to_agent(&said.by)
    ));
    news.text.push_str(&format!("\n\n(discussion_read {} reads it whole; {then}.)", d.id));
    Some(news)
}

/// Tells the agents in `d` (the one asked, the one that started it; not
/// the user, not who said it) what's said in it from `from` on.
async fn tell(slots: &SlotManager, d: &Discussion, from: usize) {
    let mut to: Vec<(&str, bool)> = vec![(d.asked.as_str(), true)];
    if d.by != d.asked {
        to.push((d.by.as_str(), false));
    }
    for (id, asked) in to.into_iter().filter(|(id, _)| *id != USER) {
        let Some(slot) = slots.get(&harness_contracts::SlotId::new(id)) else { continue };
        for at in from..d.said.len() {
            let said = &d.said[at];
            if said.by == slot.id.as_str() {
                continue;
            }
            let Some(news) = news(d, at, asked) else { continue };
            let from = format!("{} (discussion {})", d.to_agent(&said.by), d.id);
            if let Err(e) = slots.tell(&slot.id, &from, news).await {
                tracing::warn!("{} couldn't be told of discussion {}: {e}", slot.name, d.id);
            }
        }
    }
}

/// The agents in each discussion are told what's new in it by others,
/// from now on; missed events (the stream lagged): what's new since, from
/// the discussions as they are.
pub fn tell_the_asked(hub: &Arc<EventHub>, slots: &Arc<SlotManager>, discussions: &Arc<Discussions>) {
    let (_, mut events) = hub.subscribe();
    let lengths = |d: &Discussions| d.list().into_iter().map(|d| (d.id, d.said.len())).collect::<HashMap<DiscussionId, usize>>();
    let mut seen = lengths(discussions);
    let (slots, discussions) = (slots.clone(), Arc::downgrade(discussions));
    tokio::spawn(async move {
        loop {
            match events.recv().await.map(|s| s.event) {
                Ok(ClientEvent::DiscussionUpdated { discussion: d }) => {
                    let from = seen.insert(d.id.clone(), d.said.len()).unwrap_or(0);
                    tell(&slots, &d, from).await;
                }
                Ok(ClientEvent::DiscussionRemoved { discussion_id }) => {
                    seen.remove(&discussion_id);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let Some(all) = discussions.upgrade() else { return };
                    for d in all.list() {
                        let from = seen.insert(d.id.clone(), d.said.len()).unwrap_or(0);
                        tell(&slots, &d, from).await;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use nah_discussions::LEAD;

    use super::*;

    #[test]
    fn the_slot_asked_reads_what_was_said_and_how_to_answer() {
        use harness_contracts::{DiscussionStatus, Named, Said, Subject};
        // Each `#x` in what's said links tag x.
        let link = |text: &str| {
            text.find("#x").map(|at| harness_contracts::Link {
                start: at,
                end: at + 2,
                to: harness_contracts::Linked::Tag { tag: harness_contracts::TagId::new("x") },
                include: harness_contracts::Include::Auto,
            })
        };
        let said = |by: &str, kind, text: &str| Said {
            by: by.into(),
            at_ms: 0,
            kind,
            text: text.into(),
            links: link(text).into_iter().collect(),
            changed: Vec::new(),
        };
        let d = Discussion {
            id: DiscussionId::new("d3"),
            on: Named { subject: Subject::Plan { plan: arch_graph::PlanId::new("p").unwrap() }, what: "plan p".into() },
            kept: None,
            kind: DiscussionKind::ChangeRequest,
            by: USER.into(),
            asked: LEAD.into(),
            status: DiscussionStatus::Open,
            said: vec![
                said(USER, SaidKind::Message, "Split it."),
                said("lead", SaidKind::Answer, "Split #x."),
                said(USER, SaidKind::Reopened, "Not yet: #x."),
            ],
            names: Default::default(),
        };
        let first = news(&d, 0, true).unwrap().text;
        assert!(
            first.starts_with("Discussion d3 on plan p: the user asks you for a change:\n\nSplit it.")
                && first.contains("naming what you changed"),
            "{first}"
        );
        assert!(news(&d, 1, true).is_none(), "an answer isn't news to the one asked");
        // The user's links, moved with its text: still on what it says.
        let reopened = news(&d, 2, true).unwrap();
        assert!(reopened.text.contains("the user reopened it:\n\nNot yet: #x."), "{}", reopened.text);
        let l = &reopened.links[0];
        assert_eq!(reopened.text.chars().skip(l.start).take(l.end - l.start).collect::<String>(), "#x");
        // To the agent that started one: what's said back, the answer too.
        assert!(news(&d, 0, false).is_none(), "what it said itself");
        let answered = news(&d, 1, false).unwrap();
        assert!(
            answered.text.contains("lead answered it:\n\nSplit #x.") && answered.text.contains("discussion_reply says something in it")
        );
        assert!(answered.links.is_empty(), "an agent's message: plain text");
    }
}
