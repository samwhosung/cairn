use std::collections::HashMap;
use std::ops::RangeInclusive;

use dbc::FieldType;
use mpq::Chain;

use crate::Error;
use crate::equipment::{SLOT_BOOTS, SLOT_CHEST, SLOT_GLOVES, SLOT_PANTS, SLOT_SHIRT, SLOT_TABARD};
use crate::table::{parse, read_table, u32_at};

const CHAR_HAIR_GEOSETS: &str = "DBFilesClient\\CharHairGeosets.dbc";
const CHAR_FACIAL_HAIR: &str = "DBFilesClient\\CharacterFacialHairStyles.dbc";
const HELMET_GEOSET_VIS: &str = "DBFilesClient\\HelmetGeosetVisData.dbc";

const HAIR_COLUMNS: [(&str, FieldType); 6] = [
    ("ID", FieldType::UInt32),
    ("RaceID", FieldType::UInt32),
    ("SexID", FieldType::UInt32),
    ("VariationID", FieldType::UInt32),
    ("GeosetID", FieldType::UInt32),
    ("ShowScalp", FieldType::UInt32),
];

const FACIAL_HAIR_COLUMNS: [(&str, FieldType); 9] = [
    ("RaceID", FieldType::UInt32),
    ("SexID", FieldType::UInt32),
    ("VariationID", FieldType::UInt32),
    ("Unused3", FieldType::UInt32),
    ("Unused4", FieldType::UInt32),
    ("Unused5", FieldType::UInt32),
    ("Geoset100", FieldType::UInt32),
    ("Geoset300", FieldType::UInt32),
    ("Geoset200", FieldType::UInt32),
];

/// Each mask column is a bitmask of races, `1 << race`.
const HELMET_VIS_COLUMNS: [(&str, FieldType); 6] = [
    ("ID", FieldType::UInt32),
    ("HideHair", FieldType::UInt32),
    ("HideFacial1", FieldType::UInt32),
    ("HideFacial2", FieldType::UInt32),
    ("HideFacial3", FieldType::UInt32),
    ("HideEars", FieldType::UInt32),
];

const BARE_BODY_GEOSETS: [u16; 16] = [
    1, 101, 201, 301, 401, 501, 601, 702, 801, 901, 1001, 1101, 1201, 1301, 1401, 1501,
];

/// The group each helmet vis column hides, by putting it back to its first geoset: the ears
/// are group 7, whose bare look is 702.
const HELMET_HIDES: [usize; 5] = [0, 1, 2, 3, 7];

const BARE_SCALP: u32 = 1;
const GLOVES: u16 = 401;
const BOOTS: u16 = 501;
const SLEEVES: u16 = 801;
const KNEES: u16 = 901;
const DOUBLET: u16 = 1001;
const TROUSER_LEGS: u16 = 1102;
const TABARD: u16 = 1201;
const ROBE_SKIRT: u16 = 1301;
const CLOAK: u16 = 1501;

const GLOVE_GROUP: RangeInclusive<u16> = 401..=499;
const BOOT_GROUP: RangeInclusive<u16> = 501..=599;
const KNEE_GROUP: RangeInclusive<u16> = 902..=999;
const TROUSER_GROUP: RangeInclusive<u16> = 1100..=1199;
const ROBE_GROUP: RangeInclusive<u16> = 1300..=1399;
const CLOAK_GROUP: RangeInclusive<u16> = 1500..=1599;

/// An item's geoset group that makes it a robe.
const ROBE: usize = 2;

/// Race, sex and variation.
type Key = (u8, u8, u8);

/// The tables that turn an appearance and its equipment into the geosets a character shows.
pub struct CharacterGeosets {
    hair: HashMap<Key, u32>,
    facial: HashMap<Key, [u32; 3]>,
    helmet_vis: HashMap<u32, [u32; 5]>,
}

/// What the worn equipment gives the geoset choice; the default is naked.
#[derive(Default, Clone, Copy)]
pub struct EquipGeosets {
    /// Each worn item's `ItemDisplayInfo` geoset groups, by body slot less 2: shirt, chest,
    /// belt, pants, boots, wrist, gloves, tabard. `None` for an empty slot.
    pub bodyslots: [Option<[u32; 3]>; 8],
    /// The cloak's first geoset group.
    pub cloak: Option<u32>,
    /// The worn helm's `HelmetGeosetVisData` rows, `[male, female]`.
    pub helm_vis: Option<[u32; 2]>,
    /// Whether something other than the shirt dresses the forearm, from
    /// [`forearm_dressed`](crate::forearm_dressed); the shirt's cuff shows only when nothing does.
    pub forearm_dressed: bool,
    /// Whether the guild tabard designer is open: the body then wears the tabard flap, even over
    /// an empty tabard slot, unless a robe hides it.
    pub tabard_preview: bool,
}

impl CharacterGeosets {
    /// The geosets a character of this appearance and equipment shows, sorted and without
    /// repeats. A body submesh is drawn when its geoset is in the list.
    pub fn visible_geosets(
        &self,
        race: u8,
        sex: u8,
        hair_style: u8,
        facial_hair: u8,
        equip: &EquipGeosets,
    ) -> Vec<u16> {
        let mut set = BARE_BODY_GEOSETS.to_vec();
        set.push(0);
        if let Some(&g) = self.hair.get(&(race, sex, hair_style)) {
            set[0] = g.max(BARE_SCALP) as u16;
        }
        if let Some(&[one, two, three]) = self.facial.get(&(race, sex, facial_hair)) {
            set[1] = (one + 100) as u16;
            set[3] = (three + 300) as u16;
            set[2] = (two + 200) as u16;
        }
        if let Some(rows) = equip.helm_vis {
            let row = rows[usize::from(sex == 1)];
            if let Some(masks) = self.helmet_vis.get(&row) {
                let bit = 1u32 << (race & 0x1f);
                for (mask, group) in masks.iter().zip(HELMET_HIDES) {
                    if mask & bit != 0 {
                        set[group] = (group * 100 + 1) as u16;
                    }
                }
            }
        }
        let g = |slot: usize, group: usize| {
            equip.bodyslots[slot]
                .map(|groups| groups[group])
                .filter(|v| *v != 0)
        };
        let disable = |set: &mut Vec<u16>, r: RangeInclusive<u16>| set.retain(|id| !r.contains(id));
        let chest_robe = g(SLOT_CHEST, ROBE);
        let robe = chest_robe.or_else(|| g(SLOT_PANTS, ROBE));
        if let Some(v) = g(SLOT_GLOVES, 0) {
            disable(&mut set, GLOVE_GROUP);
            set.push(GLOVES + v as u16);
        } else if let Some(v) = g(SLOT_CHEST, 0) {
            set.push(SLEEVES + v as u16);
        }
        if !equip.forearm_dressed
            && let Some(v) = g(SLOT_SHIRT, 0)
        {
            set.push(SLEEVES + v as u16);
        }
        if let Some(v) = robe {
            disable(&mut set, BOOT_GROUP);
            disable(&mut set, KNEE_GROUP);
            disable(&mut set, TROUSER_GROUP);
            disable(&mut set, ROBE_GROUP);
            set.push(ROBE_SKIRT + v as u16);
        } else if let Some(v) = g(SLOT_BOOTS, 0) {
            disable(&mut set, BOOT_GROUP);
            set.push(KNEES);
            set.push(BOOTS + v as u16);
        } else if let Some(v) = g(SLOT_PANTS, 1) {
            set.push(KNEES + v as u16);
        } else {
            set.push(KNEES);
        }
        if robe.is_none()
            && let Some(v) = g(SLOT_TABARD, 0)
        {
            set.push(TABARD + v as u16);
        }
        if equip.tabard_preview {
            set.push(TABARD);
            if robe.is_none() {
                set.push(TABARD + 1);
            }
        }
        if chest_robe.is_none() && g(SLOT_TABARD, 0).is_none() {
            if let Some(v) = g(SLOT_SHIRT, 1) {
                set.push(DOUBLET + v as u16);
            }
            if let Some(v) = g(SLOT_PANTS, 0) {
                set.push(TROUSER_LEGS + v as u16);
            }
        }
        if let Some(v) = equip.cloak.filter(|v| *v != 0) {
            disable(&mut set, CLOAK_GROUP);
            set.push(CLOAK + v as u16);
        }
        set.sort_unstable();
        set.dedup();
        set
    }

    /// Reads `CharHairGeosets`, `CharacterFacialHairStyles` and, when the chain has a readable
    /// copy, `HelmetGeosetVisData`; without one, helms hide nothing. A key that repeats in the
    /// first two takes its first row, as the client's front-to-back scan finds it.
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let rs = read_table(chain, CHAR_HAIR_GEOSETS, &HAIR_COLUMNS)?;
        let mut hair = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            if let (Some(race), Some(sex), Some(var), Some(geoset)) =
                (u32_at(r, 1), u32_at(r, 2), u32_at(r, 3), u32_at(r, 4))
            {
                hair.entry((race as u8, sex as u8, var as u8))
                    .or_insert(geoset);
            }
        }
        let rs = read_table(chain, CHAR_FACIAL_HAIR, &FACIAL_HAIR_COLUMNS)?;
        let mut facial = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            if let (Some(race), Some(sex), Some(var)) = (u32_at(r, 0), u32_at(r, 1), u32_at(r, 2)) {
                let groups = [6, 8, 7].map(|i| u32_at(r, i).unwrap_or(0));
                facial
                    .entry((race as u8, sex as u8, var as u8))
                    .or_insert(groups);
            }
        }
        let helmet_vis = match chain.read(HELMET_GEOSET_VIS) {
            Ok(bytes) => {
                let rs = parse(HELMET_GEOSET_VIS, &bytes, &HELMET_VIS_COLUMNS)?;
                let mut m = HashMap::with_capacity(rs.records().len());
                for r in rs.records() {
                    if let Some(id) = u32_at(r, 0) {
                        m.insert(id, std::array::from_fn(|i| u32_at(r, 1 + i).unwrap_or(0)));
                    }
                }
                m
            }
            Err(_) => HashMap::new(),
        };
        Ok(Self {
            hair,
            facial,
            helmet_vis,
        })
    }
}

#[cfg(test)]
mod tests;
