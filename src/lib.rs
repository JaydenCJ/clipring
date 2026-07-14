//! clipring — terminal clipboard history over OSC 52.
//!
//! Capture, browse, and re-paste clipboard entries from any shell, with the
//! actual copy performed by the *terminal emulator* via OSC 52 — so it works
//! identically on a local shell, three SSH hops deep, and inside tmux or
//! GNU screen. Everything is offline; state is a JSONL file on disk.

pub mod base64;
pub mod cli;
pub mod jsonl;
pub mod osc52;
pub mod picker;
pub mod ring;
pub mod store;
pub mod textutil;

/// Crate version, single source of truth for `--version` and `info`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
