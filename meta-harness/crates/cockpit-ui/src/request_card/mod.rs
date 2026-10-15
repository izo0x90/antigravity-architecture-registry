//! What an agent asks, drawn the same wherever it's answered (the chat,
//! the inbox): an approval (what the tool call would do; allow or deny), a
//! question (its answers to pick from, or a form), or a page to open. Each
//! answer shows the key that sends it; the one clicked is handed back, to be
//! sent the way a key sends it, and where each answer is drawn, for a
//! cursor to move over.

mod answers;
mod drafts;
mod draw;

pub use answers::{Answer, answers};
pub use drafts::Drafts;
pub use draw::{Drawn, Shown, render_request};

#[cfg(test)]
mod tests;
