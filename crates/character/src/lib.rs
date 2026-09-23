//! Characters and creatures of World of Warcraft 1.12.1: what their customization selects, the skin the client composites for them, and the item and creature displays they wear.

mod creatures;
mod customization;
mod emblem;
mod equipment;
mod error;
mod geosets;
mod items;
mod sections;
mod table;
mod texture;

pub use creatures::{CreatureCatalog, CreatureModel, NpcAppearance};
pub use customization::{CharCreateCatalog, DialRanges, StartOutfitItem};
pub use emblem::{EmblemLayer, GuildEmblem};
pub use equipment::{
    BlitSource, EquipBlit, equip_blits, equip_column, equip_region_candidates, equip_tex_dir,
    equip_tile, forearm_dressed,
};
pub use error::Error;
pub use geosets::{CharacterGeosets, EquipGeosets};
pub use items::{ItemDisplay, ItemDisplayCatalog};
pub use sections::CharSections;
pub use texture::{MipChain, read_mip_chain};
