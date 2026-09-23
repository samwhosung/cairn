//! Reads the client's lighting tables and samples them into fog, sun, sky and water colours.

mod atmosphere;
mod bands;
mod catalog;
mod error;
mod report;
mod sample;
mod submersion;
mod table;

pub use atmosphere::{Atmosphere, ZERO_KEY_COLOR, ZERO_KEY_SCALAR};
pub use catalog::LightCatalog;
pub use error::Error;
pub use submersion::Submersion;
