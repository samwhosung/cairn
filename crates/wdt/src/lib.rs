//! Reads World of Warcraft 1.12.1 WDT map tables and maps world coordinates to tiles.

mod coords;
mod error;
mod reader;

pub use coords::{tile_to_world, world_to_chunk, world_to_tile};
pub use error::Error;
pub use reader::{GlobalWmo, TileInfo, WdtFile, WdtReader};
