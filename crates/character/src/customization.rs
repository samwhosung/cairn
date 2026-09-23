use std::collections::{HashMap, HashSet};

use dbc::FieldType;
use mpq::Chain;

use crate::Error;
use crate::sections::{FLAG_UNSELECTABLE, SECTION_FACE, SECTION_HAIR, SECTION_SKIN, SectionKey};
use crate::table::{nonempty_str_at, read, read_table, u32_at, unnamed_u32_columns};

const CHR_RACES: &str = "DBFilesClient\\ChrRaces.dbc";
const CHAR_BASE_INFO: &str = "DBFilesClient\\CharBaseInfo.dbc";
const CHAR_START_OUTFIT: &str = "DBFilesClient\\CharStartOutfit.dbc";
const CHAR_HAIR_GEOSETS: &str = "DBFilesClient\\CharHairGeosets.dbc";
const CHAR_FACIAL_HAIR_STYLES: &str = "DBFilesClient\\CharacterFacialHairStyles.dbc";
const CHAR_SECTIONS: &str = "DBFilesClient\\CharSections.dbc";

const PLAYABLE_RACES: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

const SHIPPED_RACE_FILES: [(u8, &str); 8] = [
    (1, "Human"),
    (2, "Orc"),
    (3, "Dwarf"),
    (4, "NightElf"),
    (5, "Scourge"),
    (6, "Tauren"),
    (7, "Gnome"),
    (8, "Troll"),
];

const SHIPPED_RACE_CLASSES: [(u8, &[u8]); 8] = [
    (1, &[1, 2, 4, 5, 8, 9]),
    (2, &[1, 3, 4, 7, 9]),
    (3, &[1, 2, 3, 4, 5, 8]),
    (4, &[1, 3, 4, 5, 11]),
    (5, &[1, 4, 5, 8, 9]),
    (6, &[1, 3, 7, 11]),
    (7, &[1, 4, 8, 9]),
    (8, &[1, 3, 4, 5, 7, 8]),
];

/// Dwarf Mage: both tables carry it, and the client never offers it.
const UNUSED_COMBOS: [(u8, u8); 1] = [(3, 8)];

const HUMAN_WARRIOR_MALE_OUTFIT: [StartOutfitItem; 5] = [
    StartOutfitItem::new(9891, 4),
    StartOutfitItem::new(9892, 7),
    StartOutfitItem::new(10141, 8),
    StartOutfitItem::new(1542, 21),
    StartOutfitItem::new(18730, 14),
];

/// The header counts each byte at 4..8 as a field: 41 fields in a 152-byte record.
const OUTFIT_FIELDS: u32 = 41;
const OUTFIT_RECORD: usize = 152;
const OUTFIT_RACE: usize = 4;
const OUTFIT_CLASS: usize = 5;
const OUTFIT_SEX: usize = 6;
const OUTFIT_DISPLAYS: usize = 56;
const OUTFIT_INVENTORY_TYPES: usize = 104;
const OUTFIT_SLOTS: usize = 12;

/// One worn item of a starting outfit: its `ItemDisplayInfo` id and its inventory type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartOutfitItem {
    pub display_id: u32,
    pub inv_type: u8,
}

impl StartOutfitItem {
    const fn new(display_id: u32, inv_type: u8) -> Self {
        Self {
            display_id,
            inv_type,
        }
    }
}

/// How many values each customization dial of a race and sex has; a dial takes `0..count`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialRanges {
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

struct Tokens {
    facial_hair: [String; 2],
    hair: String,
}

/// Each playable race's `ChrRaces` columns.
#[derive(Default)]
struct Races {
    displays: HashMap<u8, [u32; 2]>,
    files: HashMap<u8, String>,
    tokens: HashMap<u8, Tokens>,
}

type StartOutfits = HashMap<(u8, u8, u8), Vec<StartOutfitItem>>;

/// Race and sex to a table's variations.
type Variations = HashMap<(u8, u8), HashSet<u8>>;

/// Character creation: each race's body displays, names and dial labels, the classes it may
/// be, the dials' ranges, and the outfit each class starts in.
pub struct CharCreateCatalog {
    races: Races,
    combos: HashSet<(u8, u8)>,
    ranges: HashMap<(u8, u8), DialRanges>,
    start_outfits: StartOutfits,
}

impl CharCreateCatalog {
    /// The body display id of a race and sex (0 male, anything else female).
    pub fn body_display(&self, race: u8, sex: u8) -> Option<u32> {
        self.races
            .displays
            .get(&race)
            .map(|d| d[usize::from(sex != 0)])
    }

    pub fn allows(&self, race: u8, class: u8) -> bool {
        self.combos.contains(&(race, class))
    }

    /// The race's `ChrRaces` file name (`"Human"`, `"Scourge"`), which UI strings are keyed on.
    pub fn race_file(&self, race: u8) -> Option<&str> {
        self.races.files.get(&race).map(String::as_str)
    }

    /// The token naming the race's hair dials (`"NORMAL"`, `"HORNS"`).
    pub fn hair_customization(&self, race: u8) -> Option<&str> {
        self.races.tokens.get(&race).map(|t| t.hair.as_str())
    }

    /// The token naming the race and sex's facial hair dial (`"NORMAL"`, `"MARKINGS"`); `"NONE"`
    /// hides the dial.
    pub fn facial_hair_customization(&self, race: u8, sex: u8) -> Option<&str> {
        self.races
            .tokens
            .get(&race)
            .map(|t| t.facial_hair[(sex as usize).min(1)].as_str())
    }

    /// The classes a race may be created as, ascending.
    pub fn classes_for_race(&self, race: u8) -> Vec<u8> {
        let mut cs: Vec<u8> = self
            .combos
            .iter()
            .filter(|&&(r, _)| r == race)
            .map(|&(_, c)| c)
            .collect();
        cs.sort_unstable();
        cs
    }

    pub fn ranges(&self, race: u8, sex: u8) -> Option<DialRanges> {
        self.ranges.get(&(race, sex)).copied()
    }

    /// The worn items a race, class and sex starts in; empty when the table has no row for them.
    pub fn start_outfit(&self, race: u8, class: u8, sex: u8) -> &[StartOutfitItem] {
        self.start_outfits
            .get(&(race, class, sex))
            .map_or(&[], Vec::as_slice)
    }

    /// Reads `ChrRaces`, `CharBaseInfo`, `CharStartOutfit`, `CharSections`, `CharHairGeosets` and
    /// `CharacterFacialHairStyles`, then checks them against rows every shipped copy has, so a
    /// misread layout fails here rather than dressing characters wrong.
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let races = load_races(chain)?;
        let combos = load_combos(chain)?;
        let start_outfits = load_start_outfits(chain)?;
        let selectable = load_selectable_sections(chain)?;
        let hair_styles = load_variations::<6>(chain, CHAR_HAIR_GEOSETS, 1)?;
        let facial = load_variations::<9>(chain, CHAR_FACIAL_HAIR_STYLES, 0)?;
        let mut ranges = HashMap::new();
        for race in PLAYABLE_RACES {
            for sex in [0u8, 1] {
                ranges.insert(
                    (race, sex),
                    derive_ranges(race, sex, &selectable, &hair_styles, &facial),
                );
            }
        }
        let mut catalog = Self {
            races,
            combos,
            ranges,
            start_outfits,
        };
        catalog.check()?;
        for combo in UNUSED_COMBOS {
            catalog.combos.remove(&combo);
        }
        Ok(catalog)
    }

    fn check(&self) -> Result<(), Error> {
        for (race, known) in SHIPPED_RACE_CLASSES {
            let got = self.classes_for_race(race);
            if got != known {
                return Err(Error::Classes { race, got, known });
            }
        }
        for (race, known) in SHIPPED_RACE_FILES {
            if self.race_file(race) != Some(known) {
                let got = self.race_file(race).map(str::to_owned);
                return Err(Error::RaceFile { race, got, known });
            }
        }
        let outfit = self.start_outfit(1, 1, 0);
        for item in HUMAN_WARRIOR_MALE_OUTFIT {
            if !outfit.contains(&item) {
                return Err(Error::StartOutfit {
                    display_id: item.display_id,
                    inv_type: item.inv_type,
                    got: outfit.to_vec(),
                });
            }
        }
        for race in PLAYABLE_RACES {
            for sex in [0u8, 1] {
                if self.body_display(race, sex).unwrap_or(0) == 0 {
                    return Err(Error::NoBodyDisplay { race, sex });
                }
                if self.facial_hair_customization(race, sex).is_none()
                    || self.hair_customization(race).is_none()
                {
                    return Err(Error::NoCustomizationTokens { race });
                }
                if let Some(r) = self.ranges(race, sex)
                    && [r.skin, r.face, r.hair_style, r.hair_color, r.facial_hair].contains(&0)
                {
                    return Err(Error::EmptyDial {
                        race,
                        sex,
                        ranges: r,
                    });
                }
            }
        }
        Ok(())
    }
}

fn load_races(chain: &Chain) -> Result<Races, Error> {
    const MALE_DISPLAY: usize = 4;
    const FEMALE_DISPLAY: usize = 5;
    const FILE: usize = 15;
    const FACIAL_HAIR_TOKENS: [usize; 2] = [26, 27];
    const HAIR_TOKEN: usize = 28;
    let mut columns = unnamed_u32_columns::<29>();
    for i in [
        FILE,
        FACIAL_HAIR_TOKENS[0],
        FACIAL_HAIR_TOKENS[1],
        HAIR_TOKEN,
    ] {
        columns[i].1 = FieldType::String;
    }
    let rs = read_table(chain, CHR_RACES, &columns)?;
    let mut races = Races::default();
    for r in rs.records() {
        let Some(race) = u32_at(r, 0) else { continue };
        let race = race as u8;
        if !PLAYABLE_RACES.contains(&race) {
            continue;
        }
        if let (Some(male), Some(female)) = (u32_at(r, MALE_DISPLAY), u32_at(r, FEMALE_DISPLAY)) {
            races.displays.insert(race, [male, female]);
        }
        if let Some(file) = nonempty_str_at(&rs, r, FILE) {
            races.files.insert(race, file);
        }
        if let (Some(male), Some(female), Some(hair)) = (
            nonempty_str_at(&rs, r, FACIAL_HAIR_TOKENS[0]),
            nonempty_str_at(&rs, r, FACIAL_HAIR_TOKENS[1]),
            nonempty_str_at(&rs, r, HAIR_TOKEN),
        ) {
            let tokens = Tokens {
                facial_hair: [male, female],
                hair,
            };
            races.tokens.insert(race, tokens);
        }
    }
    Ok(races)
}

fn byte_packed_records<'a>(
    table: &'static str,
    bytes: &'a [u8],
    fields: u32,
    record_size: usize,
) -> Result<(usize, &'a [u8]), Error> {
    if bytes.len() < 20 || &bytes[..4] != b"WDBC" {
        return Err(Error::NotWdbc { table });
    }
    let word =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let (got_fields, got_size) = (word(8), word(12));
    if got_fields != fields || got_size as usize != record_size {
        return Err(Error::Layout {
            table,
            fields: got_fields,
            record_size: got_size,
        });
    }
    Ok((word(4) as usize, &bytes[20..]))
}

fn byte_packed_record<'a>(
    table: &'static str,
    records: &'a [u8],
    i: usize,
    size: usize,
) -> Result<&'a [u8], Error> {
    records
        .get(i * size..i * size + size)
        .ok_or(Error::RecordPastEnd { table, record: i })
}

fn load_combos(chain: &Chain) -> Result<HashSet<(u8, u8)>, Error> {
    let bytes = read(chain, CHAR_BASE_INFO)?;
    let (count, records) = byte_packed_records(CHAR_BASE_INFO, &bytes, 2, 2)?;
    let mut combos = HashSet::new();
    for i in 0..count {
        let rec = byte_packed_record(CHAR_BASE_INFO, records, i, 2)?;
        let (race, class) = (rec[0], rec[1]);
        combos.insert((race, class));
    }
    Ok(combos)
}

fn load_start_outfits(chain: &Chain) -> Result<StartOutfits, Error> {
    let bytes = read(chain, CHAR_START_OUTFIT)?;
    let (count, records) =
        byte_packed_records(CHAR_START_OUTFIT, &bytes, OUTFIT_FIELDS, OUTFIT_RECORD)?;
    let mut outfits = HashMap::new();
    for i in 0..count {
        let rec = byte_packed_record(CHAR_START_OUTFIT, records, i, OUTFIT_RECORD)?;
        let word = |at: usize| i32::from_le_bytes([rec[at], rec[at + 1], rec[at + 2], rec[at + 3]]);
        let worn = (0..OUTFIT_SLOTS)
            .map(|slot| {
                (
                    word(OUTFIT_DISPLAYS + slot * 4),
                    word(OUTFIT_INVENTORY_TYPES + slot * 4),
                )
            })
            .filter(|&(display, inv_type)| display >= 1 && inv_type >= 1)
            .map(|(display, inv_type)| StartOutfitItem::new(display as u32, inv_type as u8))
            .collect();
        let key = (rec[OUTFIT_RACE], rec[OUTFIT_CLASS], rec[OUTFIT_SEX]);
        outfits.insert(key, worn);
    }
    Ok(outfits)
}

fn load_selectable_sections(chain: &Chain) -> Result<HashSet<SectionKey>, Error> {
    let rs = read_table(chain, CHAR_SECTIONS, &unnamed_u32_columns::<10>())?;
    let mut selectable = HashSet::new();
    for r in rs.records() {
        if let (Some(key), Some(flags)) = (SectionKey::of(r), u32_at(r, 9))
            && flags & FLAG_UNSELECTABLE == 0
        {
            selectable.insert(key);
        }
    }
    Ok(selectable)
}

fn load_variations<const N: usize>(
    chain: &Chain,
    table: &'static str,
    race_column: usize,
) -> Result<Variations, Error> {
    let rs = read_table(chain, table, &unnamed_u32_columns::<N>())?;
    let mut map: Variations = HashMap::new();
    for r in rs.records() {
        if let (Some(race), Some(sex), Some(variation)) = (
            u32_at(r, race_column),
            u32_at(r, race_column + 1),
            u32_at(r, race_column + 2),
        ) {
            map.entry((race as u8, sex as u8))
                .or_default()
                .insert(variation as u8);
        }
    }
    Ok(map)
}

fn distinct(
    selectable: &HashSet<SectionKey>,
    race: u8,
    sex: u8,
    section: u8,
    axis: fn(&SectionKey) -> u8,
) -> u8 {
    selectable
        .iter()
        .filter(|k| k.race == race && k.sex == sex && k.section == section)
        .map(axis)
        .collect::<HashSet<u8>>()
        .len() as u8
}

fn derive_ranges(
    race: u8,
    sex: u8,
    selectable: &HashSet<SectionKey>,
    hair_styles: &Variations,
    facial: &Variations,
) -> DialRanges {
    let variation = |k: &SectionKey| k.variation;
    let color = |k: &SectionKey| k.color;
    DialRanges {
        skin: distinct(selectable, race, sex, SECTION_SKIN, color),
        face: distinct(selectable, race, sex, SECTION_FACE, variation),
        // The client counts hair styles in `CharHairGeosets`, and a race with none still has
        // the bald one.
        hair_style: hair_styles
            .get(&(race, sex))
            .map_or(1, |v| v.len().max(1) as u8),
        hair_color: distinct(selectable, race, sex, SECTION_HAIR, color),
        facial_hair: facial.get(&(race, sex)).map_or(0, |v| v.len() as u8),
    }
}
