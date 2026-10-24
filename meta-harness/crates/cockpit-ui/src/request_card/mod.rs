//! What an agent asks, drawn the same wherever it's answered (the chat,
//! the inbox): an approval (what the tool call would do), a question (or a
//! form), or a page to open. Its answers are actions with their keys (allow
//! or deny; a question's answers to pick from), shown under it by the host
//! while the cursor is on it, and sent the way a key sends them.

mod answers;
mod drafts;
mod draw;

pub use answers::{Answer, Offer, answers, keyed, offer, page, place_page};
pub use drafts::Drafts;
pub use draw::render_request;

#[cfg(test)]
mod tests;
