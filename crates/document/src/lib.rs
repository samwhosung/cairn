//! A zone of its own as a document: commands change it, a journal keeps every change with its
//! author, each author undoes and redoes their own steps, and it writes the tiles cairn draws.
//!
//! A command has a text form, an author and a footprint; it applies whole or not at all, and a
//! batch of commands is one step. The journal is written as each command applies, so a zone
//! reopened after a crash is the zone as of its journal's last line. Undo restores what a step's
//! footprint held before it, never replays, and is refused where another author has changed that
//! footprint since. Ids are an author's name and count, and every choice that looks random is
//! keyed by place, so an edit changes nothing outside its reach.

pub mod build;
mod choice;
pub mod cli;
pub mod command;
mod document;
mod edit;
mod files;
pub mod frame;
mod grammar;
mod history;
mod image;
pub mod install;
mod journal;
mod query;
pub mod shape;
mod steps;
mod text;
pub mod zone;

#[cfg(test)]
mod tests;

pub use command::Command;
pub use document::{Document, Done};
pub use image::Footprint;
pub use install::{Archives, Install, ModelBox};
pub use zone::{Id, Zone};
