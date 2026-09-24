use std::collections::HashMap;

use dbc::FieldType;
use mpq::Chain;

use crate::Error;
use crate::table::{f32_at, nonempty_str_at, read_table, u32_at};

const CREATURE_MODEL_DATA: &str = "DBFilesClient\\CreatureModelData.dbc";
const CREATURE_DISPLAY_INFO: &str = "DBFilesClient\\CreatureDisplayInfo.dbc";
const CREATURE_DISPLAY_INFO_EXTRA: &str = "DBFilesClient\\CreatureDisplayInfoExtra.dbc";

const MODEL_COLUMNS: [(&str, FieldType); 16] = [
    ("ID", FieldType::UInt32),
    ("Flags", FieldType::UInt32),
    ("ModelName", FieldType::String),
    ("SizeClass", FieldType::UInt32),
    ("ModelScale", FieldType::Float32),
    ("BloodID", FieldType::UInt32),
    ("FootprintTextureID", FieldType::UInt32),
    ("FootprintTextureLength", FieldType::Float32),
    ("FootprintTextureWidth", FieldType::Float32),
    ("FootprintParticleScale", FieldType::Float32),
    ("FoleyMaterialID", FieldType::UInt32),
    ("FootstepShakeSize", FieldType::UInt32),
    ("DeathThudShakeSize", FieldType::UInt32),
    ("SoundID", FieldType::UInt32),
    ("CollisionWidth", FieldType::Float32),
    ("CollisionHeight", FieldType::Float32),
];

const DISPLAY_COLUMNS: [(&str, FieldType); 12] = [
    ("ID", FieldType::UInt32),
    ("ModelID", FieldType::UInt32),
    ("SoundID", FieldType::UInt32),
    ("ExtendedDisplayInfoID", FieldType::UInt32),
    ("CreatureModelScale", FieldType::Float32),
    ("CreatureModelAlpha", FieldType::UInt32),
    ("TextureVariation0", FieldType::String),
    ("TextureVariation1", FieldType::String),
    ("TextureVariation2", FieldType::String),
    ("SizeClass", FieldType::UInt32),
    ("BloodLevel", FieldType::UInt32),
    ("NPCSoundID", FieldType::UInt32),
];

const EXTRA_COLUMNS: [(&str, FieldType); 19] = [
    ("ID", FieldType::UInt32),
    ("Race", FieldType::UInt32),
    ("Sex", FieldType::UInt32),
    ("SkinColor", FieldType::UInt32),
    ("FaceType", FieldType::UInt32),
    ("HairStyle", FieldType::UInt32),
    ("HairColor", FieldType::UInt32),
    ("FacialHair", FieldType::UInt32),
    ("Head", FieldType::UInt32),
    ("Shoulder", FieldType::UInt32),
    ("Shirt", FieldType::UInt32),
    ("Chest", FieldType::UInt32),
    ("Belt", FieldType::UInt32),
    ("Legs", FieldType::UInt32),
    ("Boots", FieldType::UInt32),
    ("Wrist", FieldType::UInt32),
    ("Gloves", FieldType::UInt32),
    ("Tabard", FieldType::UInt32),
    ("BakeName", FieldType::String),
];

/// A creature display resolved to its model.
#[derive(Debug, Clone)]
pub struct CreatureModel {
    /// The model as `CreatureModelData` names it, `.mdx`.
    pub model_path: String,
    /// The model's scale times the display's.
    pub scale: f32,
    /// The skins for the model's three creature texture slots, without directory or extension;
    /// they sit beside the model.
    pub textures: [Option<String>; 3],
    /// How a display on a character model looks; `None` for a creature model.
    pub npc_appearance: Option<NpcAppearance>,
    /// The model's collision height in model units, before any scale.
    pub collision_height: f32,
}

/// A `CreatureDisplayInfoExtra` row: the appearance of a creature that wears a character model.
#[derive(Debug, Clone)]
pub struct NpcAppearance {
    pub race: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    /// `ItemDisplayInfo` ids by body slot: head, shoulder, shirt, chest, belt, pants, boots,
    /// wrist, gloves, tabard; 0 for an empty slot.
    pub equipment: [u32; 10],
    /// The shipped pre-baked skin under `Textures\BakedNpcTextures`, when it has one.
    pub bake_name: Option<String>,
}

struct DisplayRow {
    model_id: u32,
    extended_id: u32,
    scale: f32,
    textures: [Option<String>; 3],
}

struct ModelRow {
    path: String,
    flags: u32,
    scale: f32,
    collision_height: f32,
    footprint_texture: u32,
    footprint_length_inches: f32,
    footprint_width_inches: f32,
}

/// The print a model's feet leave: its `FootprintTextures` id, and its length along the facing and
/// width across in yards, before the unit's scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Footprint {
    pub texture: u32,
    pub length: f32,
    pub width: f32,
}

const INCHES_PER_YARD: f32 = 36.0;
/// `CreatureModelData.Flags`: the model has no breath to see.
const BREATHLESS: u32 = 0x2;

/// Creature displays by id, resolved through their models. The default is empty: every lookup
/// misses.
#[derive(Default)]
pub struct CreatureCatalog {
    display: HashMap<u32, DisplayRow>,
    models: HashMap<u32, ModelRow>,
    extra: HashMap<u32, NpcAppearance>,
}

impl CreatureCatalog {
    /// Reads `CreatureModelData` and `CreatureDisplayInfo`, and `CreatureDisplayInfoExtra`
    /// when the chain has a copy that parses; without one no display has an appearance.
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let rs = read_table(chain, CREATURE_MODEL_DATA, &MODEL_COLUMNS)?;
        let mut models = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            if let (Some(id), Some(path)) = (u32_at(r, 0), nonempty_str_at(&rs, r, 2)) {
                let row = ModelRow {
                    path,
                    flags: u32_at(r, 1).unwrap_or(0),
                    scale: f32_at(r, 4).unwrap_or(1.0),
                    collision_height: f32_at(r, 15).unwrap_or(0.0),
                    footprint_texture: u32_at(r, 6).unwrap_or(u32::MAX),
                    footprint_length_inches: f32_at(r, 7).unwrap_or(0.0),
                    footprint_width_inches: f32_at(r, 8).unwrap_or(0.0),
                };
                models.insert(id, row);
            }
        }
        let rs = read_table(chain, CREATURE_DISPLAY_INFO, &DISPLAY_COLUMNS)?;
        let mut display = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            if let (Some(id), Some(model_id)) = (u32_at(r, 0), u32_at(r, 1)) {
                let row = DisplayRow {
                    model_id,
                    extended_id: u32_at(r, 3).unwrap_or(0),
                    scale: f32_at(r, 4).unwrap_or(1.0),
                    textures: [6, 7, 8].map(|i| nonempty_str_at(&rs, r, i)),
                };
                display.insert(id, row);
            }
        }
        let extra = load_extra(chain).unwrap_or_default();
        Ok(Self {
            display,
            models,
            extra,
        })
    }

    /// A display's model; `None` when the display or its model is unknown.
    pub fn model(&self, display_id: u32) -> Option<CreatureModel> {
        let row = self.display.get(&display_id)?;
        let model = self.models.get(&row.model_id)?;
        let npc_appearance = (row.extended_id != 0)
            .then(|| self.extra.get(&row.extended_id).cloned())
            .flatten();
        Some(CreatureModel {
            model_path: model.path.clone(),
            scale: model.scale * row.scale,
            textures: row.textures.clone(),
            npc_appearance,
            collision_height: model.collision_height,
        })
    }

    /// The model's scale times the display's, the size the client gives a creature that has no
    /// scale from the server.
    pub fn model_scale(&self, display_id: u32) -> Option<f32> {
        let row = self.display.get(&display_id)?;
        let model = self.models.get(&row.model_id)?;
        Some(model.scale * row.scale)
    }

    /// The display's own scale alone, which is what scales a mount.
    pub fn display_scale(&self, display_id: u32) -> Option<f32> {
        self.display.get(&display_id).map(|r| r.scale)
    }

    /// The model's collision height in model units; world yards are this times the unit's scale.
    pub fn collision_height(&self, display_id: u32) -> Option<f32> {
        let row = self.display.get(&display_id)?;
        Some(self.models.get(&row.model_id)?.collision_height)
    }

    /// The print a display's feet leave; `None` for an unknown display or model, and for a model
    /// that leaves none, whose texture is `-1` or whose print has no size.
    pub fn footprint(&self, display_id: u32) -> Option<Footprint> {
        let row = self.display.get(&display_id)?;
        let model = self.models.get(&row.model_id)?;
        let (length, width) = (
            model.footprint_length_inches / INCHES_PER_YARD,
            model.footprint_width_inches / INCHES_PER_YARD,
        );
        (model.footprint_texture != u32::MAX && length > 0.0 && width > 0.0).then_some(Footprint {
            texture: model.footprint_texture,
            length,
            width,
        })
    }

    /// Whether a display's model breathes a visible breath, as all but the skeletons, ghosts,
    /// elementals and their kind do. An unknown display breathes.
    pub fn breathes(&self, display_id: u32) -> bool {
        self.display
            .get(&display_id)
            .and_then(|row| self.models.get(&row.model_id))
            .is_none_or(|m| m.flags & BREATHLESS == 0)
    }

    /// How many displays the catalog holds.
    pub fn len(&self) -> usize {
        self.display.len()
    }

    pub fn is_empty(&self) -> bool {
        self.display.is_empty()
    }

    /// How many character-model appearances the catalog holds.
    pub fn extra_len(&self) -> usize {
        self.extra.len()
    }
}

fn load_extra(chain: &Chain) -> Result<HashMap<u32, NpcAppearance>, Error> {
    let rs = read_table(chain, CREATURE_DISPLAY_INFO_EXTRA, &EXTRA_COLUMNS)?;
    let mut extra = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let byte = |i: usize| u32_at(r, i).unwrap_or(0) as u8;
        let appearance = NpcAppearance {
            race: byte(1),
            sex: byte(2),
            skin: byte(3),
            face: byte(4),
            hair_style: byte(5),
            hair_color: byte(6),
            facial_hair: byte(7),
            equipment: std::array::from_fn(|i| u32_at(r, 8 + i).unwrap_or(0)),
            bake_name: nonempty_str_at(&rs, r, 18),
        };
        extra.insert(id, appearance);
    }
    Ok(extra)
}
