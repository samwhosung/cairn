use std::collections::HashMap;

use dbc::{FieldType, Record};
use mpq::Chain;

use crate::equipment::{EQUIP_TILES, UNDERWEAR, equip_blits};
use crate::table::{nonempty_str_at, read_table, u32_at};
use crate::texture::{MipChain, Tile, blit_over, read_mip_chain};
use crate::{Error, GuildEmblem, ItemDisplay};

const CHAR_SECTIONS: &str = "DBFilesClient\\CharSections.dbc";

const COLUMNS: [(&str, FieldType); 10] = [
    ("ID", FieldType::UInt32),
    ("RaceID", FieldType::UInt32),
    ("SexID", FieldType::UInt32),
    ("SectionType", FieldType::UInt32),
    ("VariationIndex", FieldType::UInt32),
    ("ColorIndex", FieldType::UInt32),
    ("TextureName0", FieldType::String),
    ("TextureName1", FieldType::String),
    ("TextureName2", FieldType::String),
    ("Flags", FieldType::UInt32),
];

pub(crate) const SECTION_SKIN: u8 = 0;
pub(crate) const SECTION_FACE: u8 = 1;
const SECTION_FACIAL: u8 = 2;
pub(crate) const SECTION_HAIR: u8 = 3;
const SECTION_UNDERWEAR: u8 = 4;

pub(crate) const FLAG_UNSELECTABLE: u32 = 0x1;

const HAIR_SUBSTITUTE_VARIATION: u8 = 1;

const HEAD_UPPER: Tile = (0, 160, 128, 32);
const HEAD_LOWER: Tile = (0, 192, 128, 64);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SectionKey {
    pub(crate) race: u8,
    pub(crate) sex: u8,
    pub(crate) section: u8,
    pub(crate) variation: u8,
    pub(crate) color: u8,
}

impl SectionKey {
    /// A `CharSections` row's key, each column cut to its low byte.
    pub(crate) fn of(r: &Record) -> Option<Self> {
        Some(Self {
            race: u32_at(r, 1)? as u8,
            sex: u32_at(r, 2)? as u8,
            section: u32_at(r, 3)? as u8,
            variation: u32_at(r, 4)? as u8,
            color: u32_at(r, 5)? as u8,
        })
    }
}

/// `CharSections`: the textures a race and sex's skin, face, facial hair, hair and underwear
/// choices are drawn with.
pub struct CharSections {
    sections: HashMap<SectionKey, [Option<String>; 3]>,
}

impl CharSections {
    /// The skin a race, sex and skin colour's body is drawn with, before composing.
    pub fn skin_texture(&self, race: u8, sex: u8, skin_color: u8) -> Option<&str> {
        self.tex(race, sex, SECTION_SKIN, 0, skin_color, 0)
    }

    /// The texture a character model's extra skin batches take, loaded as it is: fur, on the
    /// races that author it.
    pub fn skin_extra_texture(&self, race: u8, sex: u8, skin_color: u8) -> Option<&str> {
        self.tex(race, sex, SECTION_SKIN, 0, skin_color, 1)
    }

    /// The hair row's own sheet for a style and colour; `None` for a bald style. Texture hair
    /// geometry with [`Self::hair_mesh_texture`].
    pub fn hair_texture(&self, race: u8, sex: u8, hair_style: u8, hair_color: u8) -> Option<&str> {
        self.tex(race, sex, SECTION_HAIR, hair_style, hair_color, 0)
    }

    /// The sheet the client binds to a character model's hair batches, which on some races
    /// also dress the beard: the style's own, or style 1's at the same colour when the style has
    /// none.
    pub fn hair_mesh_texture(
        &self,
        race: u8,
        sex: u8,
        hair_style: u8,
        hair_color: u8,
    ) -> Option<&str> {
        self.hair_texture(race, sex, hair_style, hair_color)
            .or_else(|| self.hair_texture(race, sex, HAIR_SUBSTITUTE_VARIATION, hair_color))
    }

    fn tex(
        &self,
        race: u8,
        sex: u8,
        section: u8,
        variation: u8,
        color: u8,
        col: usize,
    ) -> Option<&str> {
        let key = SectionKey {
            race,
            sex,
            section,
            variation,
            color,
        };
        self.sections.get(&key).and_then(|t| t[col].as_deref())
    }

    /// Composes a character's body skin as the client does: the skin with the face, facial hair,
    /// hair and underwear drawn over it at their tiles, then the equipment and guild tabard,
    /// every stored mip level alike. `Ok(None)` when the skin has no row; an overlay or garment
    /// texture that cannot be read is left out.
    ///
    /// `equipment` holds the worn `ItemDisplayInfo` rows by body slot less 2: shirt, chest, belt,
    /// pants, boots, wrist, gloves, tabard. See [`equip_blits`] for `emblem` and
    /// `tabard_preview`.
    #[allow(clippy::too_many_arguments)]
    pub fn composite_body(
        &self,
        chain: &Chain,
        race: u8,
        sex: u8,
        skin: u8,
        face: u8,
        facial_hair: u8,
        hair_style: u8,
        hair_color: u8,
        equipment: [Option<&ItemDisplay>; 8],
        emblem: Option<GuildEmblem>,
        tabard_preview: bool,
    ) -> Result<Option<MipChain>, Error> {
        let Some(base_path) = self.skin_texture(race, sex, skin) else {
            return Ok(None);
        };
        let mut atlas = read_mip_chain(chain, base_path)?;
        let overlays: [(u8, u8, u8, usize, Tile); 6] = [
            (SECTION_FACE, face, skin, 0, HEAD_LOWER),
            (SECTION_FACE, face, skin, 1, HEAD_UPPER),
            (SECTION_FACIAL, facial_hair, hair_color, 0, HEAD_LOWER),
            (SECTION_FACIAL, facial_hair, hair_color, 1, HEAD_UPPER),
            (SECTION_HAIR, hair_style, hair_color, 1, HEAD_LOWER),
            (SECTION_HAIR, hair_style, hair_color, 2, HEAD_UPPER),
        ];
        for (ty, var, color, col, tile) in overlays {
            if let Some(path) = self.tex(race, sex, ty, var, color, col)
                && let Ok(overlay) = read_mip_chain(chain, path)
            {
                blit_over(&mut atlas, &overlay, tile);
            }
        }
        let plan = equip_blits(&equipment, emblem, tabard_preview);
        for underwear in &UNDERWEAR {
            if underwear.covered(&plan) {
                continue;
            }
            if let Some(path) = self.tex(
                race,
                sex,
                SECTION_UNDERWEAR,
                0,
                skin,
                underwear.texture_column,
            ) && let Ok(overlay) = read_mip_chain(chain, path)
            {
                blit_over(&mut atlas, &overlay, EQUIP_TILES[underwear.layer]);
            }
        }
        for step in &plan {
            if let Some(overlay) = step
                .candidates(sex)
                .iter()
                .find_map(|path| read_mip_chain(chain, path).ok())
            {
                blit_over(&mut atlas, &overlay, EQUIP_TILES[step.layer]);
            }
        }
        Ok(Some(atlas))
    }

    /// Reads `CharSections`. A repeated key keeps its last selectable row, or its first
    /// unselectable one when it has no selectable row.
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let rs = read_table(chain, CHAR_SECTIONS, &COLUMNS)?;
        let mut sections = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            let Some(key) = SectionKey::of(r) else {
                continue;
            };
            let textures = [6, 7, 8].map(|i| nonempty_str_at(&rs, r, i));
            if u32_at(r, 9).unwrap_or(0) & FLAG_UNSELECTABLE != 0 {
                sections.entry(key).or_insert(textures);
            } else {
                sections.insert(key, textures);
            }
        }
        Ok(Self { sections })
    }
}
