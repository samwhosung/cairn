//! Reads World of Warcraft 1.12.1 MPQ archives, alone or stacked into the client's patch chain.

mod archive;
mod chain;
mod crypto;
mod error;
#[cfg(test)]
mod fixture;

pub use archive::Archive;
pub use chain::{Chain, ChainEntry};
pub use error::{ChainError, Error};
