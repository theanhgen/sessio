//! sessio as a library, so the dashboard can be built for something other than a terminal.
//!
//! The binary is a thin wrapper over this; the wasm demo on the site is the other consumer. The
//! split exists so the site can run the *real* renderer instead of a hand-written imitation that
//! drifts every time the layout changes.

pub mod cta;
pub mod discover;
pub mod git;
pub mod issues;
pub mod live;
pub mod md;
pub mod model;
pub mod parse;
pub mod rank;
pub mod resume;
pub mod safety;
pub mod search;
pub mod store;
pub mod ui;
