use std::collections::HashMap;

use dbc::{FieldType, RecordSet};
use mpq::Chain;

use crate::Error;
use crate::table::{model_path, nonempty_str_at, read_table, u32_at};

const ITEM_DISPLAY_INFO: &str = "DBFilesClient\\ItemDisplayInfo.dbc";

const FLAG_GUILD_EMBLEM_TABARD: u32 = 0x1;

const COLUMNS: [(&str, FieldType); 23] = [
    ("ID", FieldType::UInt32),
    ("ModelNameLeft", FieldType::String),
    ("ModelNameRight", FieldType::String),
    ("ModelTextureLeft", FieldType::String),
    ("ModelTextureRight", FieldType::String),
    ("Icon", FieldType::String),
    ("GeosetGroup0", FieldType::UInt32),
    ("GeosetGroup1", FieldType::UInt32),
    ("GeosetGroup2", FieldType::UInt32),
    ("Flags", FieldType::UInt32),
    ("SpellVisualID", FieldType::UInt32),
    ("ItemGroupSoundsID", FieldType::UInt32),
    ("HelmVisMale", FieldType::UInt32),
    ("HelmVisFemale", FieldType::UInt32),
    ("ArmUpperTexture", FieldType::String),
    ("ArmLowerTexture", FieldType::String),
    ("HandTexture", FieldType::String),
    ("TorsoUpperTexture", FieldType::String),
    ("TorsoLowerTexture", FieldType::String),
    ("LegUpperTexture", FieldType::String),
    ("LegLowerTexture", FieldType::String),
    ("FootTexture", FieldType::String),
    ("ItemVisualID", FieldType::UInt32),
];

/// One `ItemDisplayInfo` row: how an item looks held or worn.
#[derive(Debug, Clone, Default)]
pub struct ItemDisplay {
    /// The left and right model file names, lower-cased and named `.m2`, without the directory,
    /// which depends on the kind of item.
    pub model: [Option<String>; 2],
    /// The left and right model skins, without directory or extension. Unrelated to the model's
    /// own name.
    pub model_texture: [Option<String>; 2],
    pub geoset_groups: [u32; 3],
    /// The texture this display paints on each body region, in equipment layer order.
    pub region_textures: [Option<String>; 8],
    /// `HelmetGeosetVisData` rows, `[male, female]`; 0 for none.
    pub helmet_vis: [u32; 2],
    /// The inventory icon's path, `Interface\Icons` included, without extension.
    pub icon: Option<String>,
    /// The `ItemGroupSounds` id; 0 for none.
    pub group_sounds: u32,
    /// The `SpellVisual` a ranged spell without its own borrows from this weapon; 0 for none.
    pub spell_visual: u32,
    pub flags: u32,
    /// The `ItemVisuals` id of the display's glow; 0 or less for none.
    pub item_visual: i32,
}

impl ItemDisplay {
    /// The helmet vis rows this display hides hair, facial hair and ears with, when it is a worn
    /// helm: one that names a left model.
    pub fn worn_helm_vis(&self) -> Option<[u32; 2]> {
        self.model[0].is_some().then_some(self.helmet_vis)
    }

    /// Whether this garment's torso art gives way to its wearer's guild emblem.
    pub fn takes_guild_emblem(&self) -> bool {
        self.flags & FLAG_GUILD_EMBLEM_TABARD != 0
    }
}

/// `ItemDisplayInfo` by display id.
pub struct ItemDisplayCatalog {
    displays: HashMap<u32, ItemDisplay>,
}

impl ItemDisplayCatalog {
    /// Reads `ItemDisplayInfo`; a repeated id keeps its last row.
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        Ok(Self::from_records(&read_table(
            chain,
            ITEM_DISPLAY_INFO,
            &COLUMNS,
        )?))
    }

    pub fn from_displays(displays: HashMap<u32, ItemDisplay>) -> Self {
        Self { displays }
    }

    pub fn get(&self, display_id: u32) -> Option<&ItemDisplay> {
        self.displays.get(&display_id)
    }

    pub fn len(&self) -> usize {
        self.displays.len()
    }

    pub fn is_empty(&self) -> bool {
        self.displays.is_empty()
    }

    /// Every display with its id, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &ItemDisplay)> {
        self.displays.iter().map(|(&id, d)| (id, d))
    }

    fn from_records(rs: &RecordSet) -> Self {
        let mut displays = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            let Some(id) = u32_at(r, 0) else { continue };
            let display = ItemDisplay {
                model: [1, 2].map(|i| nonempty_str_at(rs, r, i).map(|s| model_path(&s))),
                model_texture: [nonempty_str_at(rs, r, 3), nonempty_str_at(rs, r, 4)],
                geoset_groups: [6, 7, 8].map(|i| u32_at(r, i).unwrap_or(0)),
                region_textures: std::array::from_fn(|i| nonempty_str_at(rs, r, 14 + i)),
                helmet_vis: [12, 13].map(|i| u32_at(r, i).unwrap_or(0)),
                icon: nonempty_str_at(rs, r, 5).map(|i| format!("Interface\\Icons\\{i}")),
                group_sounds: u32_at(r, 11).unwrap_or(0),
                spell_visual: u32_at(r, 10).unwrap_or(0),
                flags: u32_at(r, 9).unwrap_or(0),
                item_visual: u32_at(r, 22).unwrap_or(0) as i32,
            };
            displays.insert(id, display);
        }
        Self { displays }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::parse;

    /// The string block starts with the empty string, at offset 0.
    fn one_record_table(record: &[u32], strings: &[&str]) -> Vec<u8> {
        let mut block = vec![0u8];
        for s in strings {
            block.extend_from_slice(s.as_bytes());
            block.push(0);
        }
        let mut b = b"WDBC".to_vec();
        for word in [1, 23, 92, block.len() as u32] {
            b.extend_from_slice(&u32::to_le_bytes(word));
        }
        for word in record {
            b.extend_from_slice(&word.to_le_bytes());
        }
        b.extend_from_slice(&block);
        b
    }

    fn catalog(record: &[u32], strings: &[&str]) -> ItemDisplayCatalog {
        let rs = parse(
            ITEM_DISPLAY_INFO,
            &one_record_table(record, strings),
            &COLUMNS,
        )
        .expect("parses");
        ItemDisplayCatalog::from_records(&rs)
    }

    #[test]
    fn reads_model_and_texture_independently_and_skips_empty_columns() {
        let mut record = [0u32; 23];
        record[0] = 18730;
        record[1] = 1;
        record[3] = 1 + "Shield_Round_A_01.mdx".len() as u32 + 1;
        record[6] = 1;
        record[11] = 21;
        record[22] = u32::MAX;
        let cat = catalog(
            &record,
            &["Shield_Round_A_01.mdx", "Buckler_Damaged_A_01Purple"],
        );
        assert_eq!(cat.len(), 1);
        let d = cat.get(18730).expect("display 18730");
        assert_eq!(d.model, [Some("shield_round_a_01.m2".into()), None]);
        assert_eq!(
            d.model_texture,
            [Some("Buckler_Damaged_A_01Purple".into()), None]
        );
        assert_eq!(d.geoset_groups, [1, 0, 0]);
        assert!(d.region_textures.iter().all(Option::is_none));
        assert_eq!(
            (d.group_sounds, d.item_visual, d.icon.as_deref()),
            (21, -1, None)
        );
        assert_eq!(d.worn_helm_vis(), Some([0, 0]));
        assert!(!d.takes_guild_emblem());
        assert!(cat.get(999).is_none());
    }

    #[test]
    fn a_display_with_no_left_model_is_no_helm() {
        let mut record = [0u32; 23];
        record[2] = 1;
        record[12] = 306;
        record[13] = 306;
        let d = catalog(&record, &["Helm.mdx"])
            .get(0)
            .cloned()
            .expect("display 0");
        assert_eq!(d.helmet_vis, [306, 306]);
        assert_eq!(d.worn_helm_vis(), None);
    }
}
