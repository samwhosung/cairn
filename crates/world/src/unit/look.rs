use bevy::prelude::*;
use character::{
    CharCreateCatalog, CharSections, CharacterGeosets, CreatureCatalog, EquipGeosets, ItemDisplay,
    ItemDisplayCatalog, NpcAppearance, forearm_dressed,
};

use super::body::{CharacterDress, UnitBody, WornModel};
use crate::Install;
use crate::source::{Repeat, texture_url};
use crate::texture::rgba_image;

/// The appearance tables, read from the install once.
#[derive(Resource)]
pub struct CharacterTables {
    pub creatures: CreatureCatalog,
    pub create: CharCreateCatalog,
    pub sections: CharSections,
    pub geosets: CharacterGeosets,
    pub items: ItemDisplayCatalog,
}

impl CharacterTables {
    pub fn load(install: &Install) -> Result<Self, character::Error> {
        let chain = &install.0;
        Ok(Self {
            creatures: CreatureCatalog::load(chain)?,
            create: CharCreateCatalog::load(chain)?,
            sections: CharSections::load(chain)?,
            geosets: CharacterGeosets::load(chain)?,
            items: ItemDisplayCatalog::load(chain)?,
        })
    }
}

/// Where a character body's skin comes from: composited from its appearance, or a sheet the
/// client ships ready made under `Textures\BakedNpcTextures`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodySkin {
    Composite,
    Baked(String),
}

/// What a character looks like: race, sex, the five customization choices, where its skin comes
/// from, and the `ItemDisplayInfo` ids it wears by body slot (head, shoulder, shirt, chest, belt,
/// pants, boots, wrist, gloves, tabard).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterLook {
    pub race: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    pub body: BodySkin,
    pub equipment: [u32; 10],
}

impl CharacterLook {
    /// A naked character with a composited skin.
    pub fn naked(race: u8, sex: u8) -> Self {
        Self {
            race,
            sex,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
            body: BodySkin::Composite,
            equipment: [0; 10],
        }
    }

    fn of_npc(npc: &NpcAppearance) -> Self {
        Self {
            race: npc.race,
            sex: npc.sex,
            skin: npc.skin,
            face: npc.face,
            hair_style: npc.hair_style,
            hair_color: npc.hair_color,
            facial_hair: npc.facial_hair,
            body: npc
                .bake_name
                .clone()
                .map_or(BodySkin::Composite, BodySkin::Baked),
            equipment: npc.equipment,
        }
    }
}

impl CharacterTables {
    /// The body a creature display draws, or `None` for a display the tables do not resolve or a
    /// model scaled to nothing.
    pub fn display_body(
        &self,
        display_id: u32,
        chain: &mpq::Chain,
        images: &mut Assets<Image>,
        server: &AssetServer,
    ) -> Option<UnitBody> {
        let m = self.creatures.model(display_id).filter(|m| m.scale > 0.0)?;
        let character = m
            .npc_appearance
            .as_ref()
            .map(|npc| self.dress(&CharacterLook::of_npc(npc), chain, images, server));
        Some(UnitBody {
            display: display_id,
            model: m.model_path,
            skins: m.textures,
            character,
        })
    }

    /// A player's body: its race and sex's own display, dressed in its look. `None` when the
    /// tables have no body for the race and sex.
    pub fn player_body(
        &self,
        look: &CharacterLook,
        chain: &mpq::Chain,
        images: &mut Assets<Image>,
        server: &AssetServer,
    ) -> Option<UnitBody> {
        let display = self.create.body_display(look.race, look.sex)?;
        let m = self.creatures.model(display)?;
        Some(UnitBody {
            display,
            model: m.model_path,
            skins: m.textures,
            character: Some(self.dress(look, chain, images, server)),
        })
    }

    pub fn dress(
        &self,
        look: &CharacterLook,
        chain: &mpq::Chain,
        images: &mut Assets<Image>,
        server: &AssetServer,
    ) -> CharacterDress {
        let worn: [Option<&ItemDisplay>; 8] = std::array::from_fn(|i| {
            let id = look.equipment[i + 2];
            (id != 0).then(|| self.items.get(id)).flatten()
        });
        let mut equip = EquipGeosets::default();
        for (slot, row) in worn.iter().enumerate() {
            equip.bodyslots[slot] = row.map(|r| r.geoset_groups);
        }
        equip.forearm_dressed = forearm_dressed(&worn);
        let helm = look.equipment[0];
        if helm != 0 {
            equip.helm_vis = self.items.get(helm).and_then(ItemDisplay::worn_helm_vis);
        }
        let geosets = self.geosets.visible_geosets(
            look.race,
            look.sex,
            look.hair_style,
            look.facial_hair,
            &equip,
        );
        let load = |path: &str| server.load::<Image>(texture_url(path, Repeat::BOTH));
        let body = match &look.body {
            BodySkin::Baked(name) => Some(load(&format!("Textures\\BakedNpcTextures\\{name}"))),
            BodySkin::Composite => self
                .sections
                .composite_body(
                    chain,
                    look.race,
                    look.sex,
                    look.skin,
                    look.face,
                    look.facial_hair,
                    look.hair_style,
                    look.hair_color,
                    worn,
                    None,
                    false,
                )
                .inspect_err(|e| warn!("no composited skin: {e}"))
                .ok()
                .flatten()
                .map(|atlas| {
                    images.add(rgba_image(
                        atlas.width,
                        atlas.height,
                        atlas.mips,
                        Repeat::BOTH,
                    ))
                }),
        };
        CharacterDress {
            body,
            hair: self
                .sections
                .hair_mesh_texture(look.race, look.sex, look.hair_style, look.hair_color)
                .map(load),
            skin_extra: self
                .sections
                .skin_extra_texture(look.race, look.sex, look.skin)
                .map(load),
            object: None,
            geosets,
            worn: self.worn_models(look),
        }
    }

    /// The helm and the pauldron pair a look wears: the helm's file is the one made for the
    /// wearer's race and sex, the pauldrons a left and right model off one display.
    fn worn_models(&self, look: &CharacterLook) -> Vec<WornModel> {
        const RACE_PREFIX: [&str; 8] = ["Hu", "Or", "Dw", "Ni", "Sc", "Ta", "Gn", "Tr"];
        let mut worn = Vec::new();
        let model = |display: u32, dir: &str, col: usize, helm: bool| -> Option<WornModel> {
            let d = self.items.get(display)?;
            let mut model = d.model[col].clone()?;
            if helm {
                let prefix = RACE_PREFIX[(look.race.clamp(1, 8) - 1) as usize];
                let letter = if look.sex.min(1) == 1 { 'F' } else { 'M' };
                let stem = model.strip_suffix(".m2").unwrap_or(&model).to_owned();
                model = format!("{stem}_{prefix}{letter}.m2");
            }
            let dir = format!("Item\\ObjectComponents\\{dir}");
            Some(WornModel {
                model: format!("{dir}\\{model}"),
                object_texture: d.model_texture[col]
                    .as_ref()
                    .map(|t| format!("{dir}\\{t}.blp")),
                attachment: 0,
            })
        };
        let [helm, shoulder, ..] = look.equipment;
        if helm != 0
            && let Some(w) = model(helm, "Head", 0, true)
        {
            worn.push(WornModel {
                attachment: ATTACH_HELM,
                ..w
            });
        }
        if shoulder != 0 {
            for (col, attachment) in [(0, ATTACH_SHOULDER_LEFT), (1, ATTACH_SHOULDER_RIGHT)] {
                if let Some(w) = model(shoulder, "Shoulder", col, false) {
                    worn.push(WornModel { attachment, ..w });
                }
            }
        }
        worn
    }
}

const ATTACH_HELM: u16 = 11;
const ATTACH_SHOULDER_LEFT: u16 = 6;
const ATTACH_SHOULDER_RIGHT: u16 = 5;
