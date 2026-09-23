use crate::emblem::{EmblemLayer, GuildEmblem, emblem_half};
use crate::items::ItemDisplay;
use crate::texture::Tile;

pub(crate) const EQUIP_TILES: [Tile; 8] = [
    (0, 0, 128, 64),
    (0, 64, 128, 64),
    (0, 128, 128, 32),
    (128, 0, 128, 64),
    (128, 64, 128, 32),
    (128, 96, 128, 64),
    (128, 160, 128, 64),
    (128, 224, 128, 32),
];

const EQUIP_TEX_DIRS: [&str; 8] = [
    "ArmUpperTexture",
    "ArmLowerTexture",
    "HandTexture",
    "TorsoUpperTexture",
    "TorsoLowerTexture",
    "LegUpperTexture",
    "LegLowerTexture",
    "FootTexture",
];

const NEVER: i8 = -1;

const CELL_BY_SLOT_AND_LAYER: [[i8; 8]; 8] = [
    [0, 0, NEVER, 0, 0, NEVER, NEVER, NEVER],
    [1, 1, NEVER, 1, 1, 1, 1, NEVER],
    [NEVER, NEVER, NEVER, NEVER, NEVER, 2, NEVER, NEVER],
    [NEVER, NEVER, NEVER, NEVER, NEVER, 0, 0, NEVER],
    [NEVER, NEVER, NEVER, NEVER, NEVER, NEVER, 2, 0],
    [NEVER, 2, NEVER, NEVER, NEVER, NEVER, NEVER, NEVER],
    [NEVER, 3, 0, NEVER, NEVER, NEVER, NEVER, NEVER],
    [NEVER, NEVER, NEVER, 4, 4, NEVER, NEVER, NEVER],
];

/// The client draws underwear only when none of the first `covered_by` cells of its layer's
/// row is painted: a fallback, not a layer under the equipment.
pub(crate) struct Underwear {
    pub(crate) texture_column: usize,
    pub(crate) layer: usize,
    covered_by: i8,
}

const PANTIES: Underwear = Underwear {
    texture_column: 0,
    layer: LAYER_LEG_UPPER,
    covered_by: 2,
};

const BRA: Underwear = Underwear {
    texture_column: 1,
    layer: LAYER_TORSO_UPPER,
    covered_by: 3,
};

pub(crate) const UNDERWEAR: [Underwear; 2] = [PANTIES, BRA];

impl Underwear {
    pub(crate) fn covered(&self, plan: &[EquipBlit<'_>]) -> bool {
        plan.iter()
            .any(|s| s.layer == self.layer && s.column < self.covered_by)
    }
}

pub(crate) const SLOT_SHIRT: usize = 0;
pub(crate) const SLOT_CHEST: usize = 1;
pub(crate) const SLOT_PANTS: usize = 3;
pub(crate) const SLOT_BOOTS: usize = 4;
pub(crate) const SLOT_GLOVES: usize = 6;
pub(crate) const SLOT_TABARD: usize = 7;

const LAYER_ARM_LOWER: usize = 1;
pub(crate) const LAYER_TORSO_UPPER: usize = 3;
pub(crate) const LAYER_TORSO_LOWER: usize = 4;
const LAYER_LEG_UPPER: usize = 5;
const LAYER_LEG_LOWER: usize = 6;

/// One equipment paint of a dressed skin composite: the layer and cell it takes, and its art.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EquipBlit<'a> {
    /// The layer, which is the item's texture column and the atlas tile.
    pub layer: usize,
    /// The cell in the layer's row; cells are painted in ascending order.
    pub column: i8,
    pub source: BlitSource<'a>,
}

/// What an [`EquipBlit`] paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlitSource<'a> {
    /// A worn item's texture for the layer, by body slot less 2, without directory or suffix.
    Worn { slot: usize, texture: &'a str },
    /// One layer of the wearer's guild tabard.
    Emblem {
        part: EmblemLayer,
        emblem: GuildEmblem,
    },
}

impl EquipBlit<'_> {
    /// The files this paint may come from, in the order the composite tries them.
    pub fn candidates(&self, sex: u8) -> Vec<String> {
        match self.source {
            BlitSource::Worn { texture, .. } => {
                equip_region_candidates(self.layer, texture, sex).into()
            }
            BlitSource::Emblem { part, emblem } => emblem_half(self.layer)
                .map(|half| vec![part.path(&emblem, half)])
                .unwrap_or_default(),
        }
    }
}

/// The cell body slot `slot` less 2 takes in `layer`'s row; on the lower arm and lower leg it
/// depends on what is worn.
pub fn equip_column(equipment: &[Option<&ItemDisplay>; 8], slot: usize, layer: usize) -> i8 {
    let group = |s: usize, j: usize| equipment[s].is_some_and(|d| d.geoset_groups[j] != 0);
    match (layer, slot) {
        (LAYER_ARM_LOWER, SLOT_GLOVES) if group(slot, 0) => 6,
        (LAYER_ARM_LOWER, SLOT_CHEST) if group(slot, 0) => 5,
        (LAYER_LEG_LOWER, SLOT_BOOTS) if group(slot, 0) => 3,
        (LAYER_LEG_LOWER, SLOT_CHEST) if group(slot, 2) => 4,
        (LAYER_LEG_LOWER, SLOT_PANTS) if group(slot, 2) => {
            if group(SLOT_CHEST, 2) {
                3
            } else {
                4
            }
        }
        _ => CELL_BY_SLOT_AND_LAYER[slot][layer],
    }
}

/// The paints of a dressed composite, layer by layer and cell by cell. A worn item paints a
/// layer when its slot reaches the layer and it names a texture there; two paints landing on one
/// cell keep the later slot's.
///
/// `emblem` paints only while the tabard designer is open or over a worn tabard that takes a
/// guild emblem, and then takes cells 2, 3 and 4 of the torso layers, in place of the tabard's own
/// art.
pub fn equip_blits<'a>(
    equipment: &[Option<&'a ItemDisplay>; 8],
    emblem: Option<GuildEmblem>,
    tabard_preview: bool,
) -> Vec<EquipBlit<'a>> {
    let emblem = emblem.filter(|_| {
        tabard_preview || equipment[SLOT_TABARD].is_some_and(ItemDisplay::takes_guild_emblem)
    });
    let mut plan = Vec::new();
    for (layer, _) in EQUIP_TILES.iter().enumerate() {
        let mut row: [Option<EquipBlit<'a>>; 8] = [None; 8];
        for (slot, display) in equipment.iter().enumerate() {
            let Some(display) = display else { continue };
            if CELL_BY_SLOT_AND_LAYER[slot][layer] == NEVER {
                continue;
            }
            let Some(texture) = display.region_textures[layer]
                .as_deref()
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            let column = equip_column(equipment, slot, layer);
            if let Some(cell) = row.get_mut(column as usize) {
                *cell = Some(EquipBlit {
                    layer,
                    column,
                    source: BlitSource::Worn { slot, texture },
                });
            }
        }
        if let Some(emblem) = emblem.filter(|_| emblem_half(layer).is_some()) {
            for part in EmblemLayer::ALL {
                let column = part.column();
                if let Some(cell) = row.get_mut(column as usize) {
                    *cell = Some(EquipBlit {
                        layer,
                        column,
                        source: BlitSource::Emblem { part, emblem },
                    });
                }
            }
        }
        plan.extend(row.into_iter().flatten());
    }
    plan
}

/// Whether anything other than the shirt paints the lower arm.
pub fn forearm_dressed(equipment: &[Option<&ItemDisplay>; 8]) -> bool {
    equip_blits(equipment, None, false)
        .iter()
        .any(|b| b.layer == LAYER_ARM_LOWER && (1..=6).contains(&b.column))
}

/// The atlas tile `(x, y, width, height)` of equipment layer `layer` in the 256 × 256 body skin.
pub fn equip_tile(layer: usize) -> Option<(u32, u32, u32, u32)> {
    EQUIP_TILES.get(layer).copied()
}

/// The `Item\TextureComponents` directory equipment layer `layer` reads its art from.
pub fn equip_tex_dir(layer: usize) -> Option<&'static str> {
    EQUIP_TEX_DIRS.get(layer).copied()
}

/// The files an item's texture `name` for `layer` may come from, in the order the client tries
/// them: the unisex `_U`, then the wearer's `_M` or `_F`. Panics past layer 7.
pub fn equip_region_candidates(layer: usize, name: &str, sex: u8) -> [String; 2] {
    let dir = EQUIP_TEX_DIRS[layer];
    let letter = if sex == 1 { 'F' } else { 'M' };
    ['U', letter].map(|c| format!("Item\\TextureComponents\\{dir}\\{name}_{c}.blp"))
}

#[cfg(test)]
mod tests;
