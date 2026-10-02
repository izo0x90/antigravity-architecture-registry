//! Google Antigravity as a harness provider: its ACP server driven through
//! `harness-acp`, with Antigravity's modes, approval wording and client file access
//! supplied by [`AntigravityFlavor`].

mod driver;
mod flavor;
mod instance;
pub mod skills;

pub use driver::{AntigravityConfig, AntigravityDriver};
pub use flavor::AntigravityFlavor;
