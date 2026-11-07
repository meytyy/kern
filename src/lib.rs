//! KERN — native IDE. Everything except `ui` is independent of the GUI toolkit,
//! so that it can be tested headlessly and ported to other platforms.
pub mod agents;
pub mod core;
pub mod git;
pub mod github;
pub mod lang;
pub mod lsp;
pub mod platform;
pub mod run;
pub mod terminal;
pub mod ui;
pub mod workspace;
