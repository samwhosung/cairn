//! The client's lighting: tables sampled into fog, sun, sky and water colour, and the day curves.

mod atmosphere;
mod bands;
mod catalog;
pub mod daynight;
mod error;
mod report;
mod sample;
mod submersion;
mod table;

pub use atmosphere::{Atmosphere, ZERO_KEY_COLOR, ZERO_KEY_SCALAR};
pub use catalog::LightCatalog;
pub use error::Error;
pub use submersion::Submersion;
