//! A call's changes as the Changes view's: its two texts (as its harness
//! gave them) compared on the server as a file's sides are
//! (`/api/changes/texts`), once each, kept for every view that draws them
//! with what they add and remove. Views read it (`Reader<Comparer>`) and
//! ask for a pair not known yet (`Ask::Changes`); the shell sends what's
//! asked each frame, many in one call.

use harness_contracts::changes::Hunk;

use crate::asked::{Asked, Bounds, Found};

/// Pairs kept at the most, in one call at the most, and their text in one
/// call (bytes: the server's bound).
const BOUNDS: Bounds = Bounds { kept: 512, per_call: 32, call_bytes: harness_contracts::changes::TEXTS_MOST };

/// Two texts to compare, and the unchanged lines shown around each change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    pub old: String,
    pub new: String,
    pub context: u32,
}

/// A pair compared: its hunks, and how many lines they add and remove.
#[derive(Debug, Default)]
pub struct Compared {
    pub hunks: Vec<Hunk>,
    pub added: usize,
    pub removed: usize,
}

impl Compared {
    pub fn of(hunks: Vec<Hunk>) -> Self {
        let (added, removed) = crate::diff::counts(hunks.iter().flat_map(|h| &h.lines));
        Self { hunks, added, removed }
    }
}

pub struct Comparer {
    asked: Asked<Pair, Compared>,
}

impl Default for Comparer {
    fn default() -> Self {
        Self { asked: Asked::new(BOUNDS) }
    }
}

impl Comparer {
    /// `old` and `new` compared (with `context` lines around each
    /// change), when it's known.
    pub fn compared(&self, old: &str, new: &str, context: u32) -> Found<Compared> {
        self.asked.get(key(old, new, context))
    }

    /// Pairs a view asked for (`Ask::Changes`): sent with the next frame's
    /// call.
    pub fn want(&mut self, pairs: Vec<Pair>) {
        for pair in pairs {
            let (key, size) = (key(&pair.old, &pair.new, pair.context), pair.old.len() + pair.new.len());
            self.asked.want(key, pair, size);
        }
    }

    /// Once a frame (the shell): what came back kept, what's asked sent.
    pub fn tick(&mut self, net: &crate::net::NetClient, ctx: &egui::Context) {
        self.asked.tick(ctx.input(|i| i.time), |pairs, reply| {
            net.changes_texts(&pairs, ctx.clone(), move |result| reply.answered(result.ok().map(|each| each.into_iter().map(Compared::of).collect())));
        });
    }

    /// As if the server had compared `pair` so.
    #[cfg(test)]
    pub(crate) fn knows(&mut self, pair: &Pair, hunks: Vec<Hunk>) {
        self.asked.knows(key(&pair.old, &pair.new, pair.context), Compared::of(hunks));
    }
}

/// A pair's key, from its parts (as it's found without making one).
fn key(old: &str, new: &str, context: u32) -> crate::asked::Key {
    crate::asked::key(&(old, new, context))
}
