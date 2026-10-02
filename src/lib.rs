//! Steam-only Monster Hunter Rise save editor.
//!
//! The container, crypto, and payload layers are ported verbatim from the
//! MIT-licensed [`mhrise-save-converter`](https://github.com/jinghaihan/mhrise-save-converter)
//! project; see README.md for the full attribution. This crate adds a
//! document layer (`container`) and a lossless JSON dump/load layer (`json`)
//! on top of them.

pub mod archive;
pub mod container;
pub mod crypto;
pub mod diff;
pub mod discover;
pub mod edit;
pub mod format;
pub mod gui;
pub mod json;
pub mod payload;
