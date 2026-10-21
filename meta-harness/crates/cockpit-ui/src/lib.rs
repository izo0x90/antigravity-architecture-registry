pub mod actions;
pub mod agents;
pub mod alerts;
pub mod app;
pub mod attention;
pub mod buffer;
pub mod card;
pub mod chat;
pub mod code;
pub mod panels;
pub mod config;
pub mod conversation;
pub mod diff;
pub mod discussions;
pub mod feedback;
pub mod graph;
pub mod inbox;
pub mod changes;
pub mod graph_config;
pub mod goto;
pub mod graph_style;
pub mod keymap;
pub mod kind;
#[cfg(test)]
mod lint;
pub mod nav;
pub mod viewport;
pub mod net;
pub mod palette;
pub mod hosted;
pub mod queue;
pub mod quick_look;
pub mod remote;
pub mod request_card;
pub mod review;
pub mod saved;
pub mod shared;
pub mod spec;
pub mod status;
pub mod store;
pub mod terminal;
pub mod tag_list;
pub mod tags;
pub mod terminals;
pub mod theme;
pub mod touch;
pub mod tree;

// The browser's start-up and everything that's the browser's own: the one
// place the cockpit knows it runs in a browser.
#[cfg(target_arch = "wasm32")]
mod web;
