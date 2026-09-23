//! Reads World of Warcraft 1.12.1 ADT terrain tiles: heights, textures, liquids and placements.

mod alpha;
mod error;
mod liquid;
mod mcnk;
mod root;

pub use alpha::CombinedAlphaMap;
pub use error::Error;
pub use liquid::{LiquidVertex, MclqChunk};
pub use mcnk::{
    MCNK_IMPASSABLE, McalChunk, MclyChunk, MclyFlags, MclyLayer, McnkChunk, McnkHeader, McnrChunk,
    McshChunk, McvtChunk, VertexNormal,
};
pub use root::{DoodadPlacement, RootAdt, WmoPlacement, parse_adt};
