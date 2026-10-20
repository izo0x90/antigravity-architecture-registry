//! Discussions and the agents' slots: who a discussion asks (the lead
//! unless another slot is named, or the user when an agent asks them), and
//! the slot asked told when a discussion is started, said something in or
//! reopened by anyone but itself (as a sub-slot's report is: in its running
//! session, else held until one runs).

use std::collections::HashMap;
use std::sync::Arc;

use harness_contracts::{ClientEvent, Discussion, DiscussionId, DiscussionKind, SaidKind, SlotId};
use harness_core::{EventHub, SlotManager};
use nah_discussions::{Discussions, USER};

/// Who's asked, from what was named (a slot's id or name; none: the lead;
/// `user`: the user, asked by an agent): a slot's id, or `user`.
pub fn asked(slots: &SlotManager, named: Option<&str>, by: &str) -> Result<String, String> {
    let named = named.map(str::trim).filter(|n| !n.is_empty());
    let Some(named) = named else { return Ok(slots.lead().to_string()) };
    if named == USER {
        return if by == USER { Err("you can't ask yourself: ask an agent slot (or none: the lead)".into()) } else { Ok(USER.to_owned()) };
    }
    let all = slots.list();
    if let Some(slot) = all.iter().find(|s| s.id.as_str() == named) {
        return Ok(slot.id.to_string());
    }
    match all.iter().filter(|s| s.name == named).collect::<Vec<_>>().as_slice() {
        [one] => Ok(one.id.to_string()),
        [] => Err(format!("there's no agent slot {named:?} to ask (none: the lead)")),
        _ => Err(format!("several slots are named {named:?}: name one by its id")),
    }
}

/// Who said something, as the slot asked reads it.
fn said_by(by: &str) -> String {
    if by == USER { "the user".into() } else { by.to_owned() }
}

/// What the slot asked is told of what was said (`at`: which of its
/// messages).
fn news(d: &Discussion, at: usize) -> Option<String> {
    let said = d.said.get(at)?;
    let what = match (at, said.kind, d.kind) {
        (0, _, DiscussionKind::ChangeRequest) => "asks you for a change",
        (0, _, DiscussionKind::Question) => "asks you",
        (_, SaidKind::Reopened, _) => "reopened it",
        (_, SaidKind::Message, _) => "said",
        _ => return None,
    };
    let answer = if d.kind == DiscussionKind::ChangeRequest { "discussion_answer it, naming what you changed" } else { "discussion_answer it" };
    Some(format!(
        "Discussion {} on {}: {} {what}:\n\n{}\n\n(discussion_read {} reads it whole; {answer}, or discussion_reply to ask something back.)",
        d.id,
        d.on.what,
        said_by(&said.by),
        said.text,
        d.id
    ))
}

/// The slot asked by each discussion is told what's new in it by others,
/// from now on.
pub fn tell_the_asked(hub: &Arc<EventHub>, slots: &Arc<SlotManager>, discussions: &Arc<Discussions>) {
    let (_, mut events) = hub.subscribe();
    let seen_now = |d: &Discussions| d.list().into_iter().map(|d| (d.id, d.said.len())).collect::<HashMap<DiscussionId, usize>>();
    let mut seen = seen_now(discussions);
    let (slots, discussions) = (slots.clone(), Arc::downgrade(discussions));
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(ClientEvent::DiscussionUpdated { discussion: d }) => {
                    let from = seen.insert(d.id.clone(), d.said.len()).unwrap_or(0);
                    if d.asked == USER {
                        continue;
                    }
                    let to = SlotId::new(d.asked.clone());
                    let Some(slot) = slots.get(&to) else { continue };
                    for at in from..d.said.len() {
                        if d.said[at].by == slot.name {
                            continue;
                        }
                        let Some(news) = news(&d, at) else { continue };
                        let from = format!("{} (discussion {})", said_by(&d.said[at].by), d.id);
                        if let Err(e) = slots.tell(&to, &from, news).await {
                            tracing::warn!("{} couldn't be told of discussion {}: {e}", slot.name, d.id);
                        }
                    }
                }
                Ok(ClientEvent::DiscussionRemoved { discussion_id }) => {
                    seen.remove(&discussion_id);
                }
                // Missed some: what's said meanwhile isn't told (it's in `discussions`).
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => match discussions.upgrade() {
                    Some(d) => seen = seen_now(&d),
                    None => return,
                },
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
        let said = |by: &str, kind, text: &str| Said { by: by.into(), at_ms: 0, kind, text: text.into(), changed: Vec::new() };
        let d = Discussion {
            id: DiscussionId::new("d3"),
            on: Named { subject: Subject::Plan { plan: arch_graph::PlanId::new("p").unwrap() }, what: "plan p".into() },
            kept: None,
            kind: DiscussionKind::ChangeRequest,
            by: USER.into(),
            asked: LEAD.into(),
            status: DiscussionStatus::Open,
            said: vec![said(USER, SaidKind::Message, "Split it."), said("lead", SaidKind::Answer, "Split."), said(USER, SaidKind::Reopened, "Not yet.")],
        };
        let first = news(&d, 0).unwrap();
        assert!(first.starts_with("Discussion d3 on plan p: the user asks you for a change:\n\nSplit it.") && first.contains("naming what you changed"), "{first}");
        assert!(news(&d, 1).is_none(), "an answer isn't news to the one asked");
        assert!(news(&d, 2).unwrap().contains("the user reopened it:\n\nNot yet."));
    }
}
