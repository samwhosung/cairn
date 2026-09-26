use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use adt::MCNK_OCEAN;
use atlas::Areas;
use bevy::math::Vec3;
use dbc::{DbcParser, FieldType, Record, Schema, SchemaField, Value};
use light::LightCatalog;
use terrain::CHUNK_SIZE;
use world::{Borrowed, CurrentMap, Install};

use crate::view::Pose;

pub const FILE: &str = "zone.txt";
const KEYS: [&str; 4] = ["name", "start", "facing", "borrows"];
const WORLD_MAP_AREA: &str = "DBFilesClient\\WorldMapArea.dbc";
const TILES_A_SIDE: u32 = 64;
const PAST_EVERY_MAP_DBC_ID: u32 = 0x8000_0000;
const FNV_OFFSET_BASIS: u32 = 0x811c_9dc5;
const FNV_PRIME: u32 = 0x0100_0193;

#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub root: PathBuf,
    pub directory: String,
    pub feet_wow: Vec3,
    pub facing_deg: f32,
    pub borrows: String,
}

impl Zone {
    pub fn read(root: &Path) -> Result<Self, String> {
        let path = root.join(FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!(
                "{} has no {FILE}, the file that names a zone, where it starts and the install's \
                 zone it borrows (cairn --help shows one)",
                root.display()
            ),
            _ => format!("{}: {e}", path.display()),
        })?;
        parse(root, &text)
    }

    pub fn start(&self) -> Pose {
        Pose::start(self.feet_wow, self.facing_deg)
    }

    pub fn open(&self, patched: &Install) -> Result<CurrentMap, String> {
        let chain = &patched.0;
        if let Ok(map) = CurrentMap::find(chain, &self.directory) {
            return Err(format!(
                "{}: {} is a map of the install's; a zone needs a name of its own",
                self.file(),
                map.directory
            ));
        }
        let wdt = format!("World\\Maps\\{0}\\{0}.wdt", self.directory);
        if !chain.contains(&wdt) {
            let wdt = wdt.replace('\\', "/");
            return Err(format!("{} has no {wdt}", self.root.display()));
        }
        let areas = Areas::load(chain)?;
        let area = areas.zone_named(&self.borrows, None).ok_or_else(|| {
            format!(
                "{}: the install has no zone named {}",
                self.file(),
                self.borrows
            )
        })?;
        let light = light_over_most_of(patched, &areas, area)?.ok_or_else(|| {
            format!(
                "{}: {} has no ground in the install to take its light from",
                self.file(),
                self.borrows
            )
        })?;
        Ok(CurrentMap {
            id: map_id(&self.directory),
            directory: self.directory.clone(),
            borrowed: Some(Borrowed { light, area }),
        })
    }

    fn file(&self) -> String {
        self.root.join(FILE).display().to_string()
    }
}

fn parse(root: &Path, text: &str) -> Result<Zone, String> {
    let file = root.join(FILE).display().to_string();
    let mut lines: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
    for (i, raw) in text.lines().enumerate() {
        let at = i + 1;
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("{file}:{at}: want `key = value`"));
        };
        let (key, value) = (key.trim(), value.trim());
        let Some(key) = KEYS.iter().copied().find(|k| *k == key) else {
            return Err(format!(
                "{file}:{at}: no key `{key}`: a zone has {}",
                KEYS.join(", ")
            ));
        };
        if let Some((first, _)) = lines.insert(key, (at, value)) {
            return Err(format!("{file}:{at}: `{key}` again, after line {first}"));
        }
    }
    let given = |key: &str, form: &str| {
        lines
            .get(key)
            .copied()
            .ok_or_else(|| format!("{file} names no {key}: `{key} = {form}`"))
    };
    let (at, name) = given("name", "NAME")?;
    let mut letters = name.chars();
    if !(letters.next().is_some_and(|c| c.is_ascii_alphabetic())
        && letters.all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err(format!(
            "{file}:{at}: a name is its map's directory: letters, digits and _, from a letter, \
             not `{name}`"
        ));
    }
    let (at, start) = given("start", "X, Y, Z")?;
    let feet_wow = start
        .split(',')
        .map(|n| n.trim().parse::<f32>().ok().filter(|n| n.is_finite()))
        .collect::<Option<Vec<f32>>>()
        .and_then(|n| <[f32; 3]>::try_from(n).ok())
        .map(Vec3::from_array)
        .ok_or_else(|| format!("{file}:{at}: start wants X, Y, Z in yards, not `{start}`"))?;
    let facing_deg = match lines.get("facing") {
        None => 0.0,
        Some(&(at, facing)) => facing
            .parse::<f32>()
            .ok()
            .filter(|n| n.is_finite())
            .ok_or_else(|| {
                format!("{file}:{at}: facing wants degrees from north toward west, not `{facing}`")
            })?,
    };
    let (at, borrows) = given("borrows", "ZONE")?;
    if borrows.is_empty() {
        return Err(format!(
            "{file}:{at}: borrows wants the name of a zone of the install's"
        ));
    }
    Ok(Zone {
        root: root.to_path_buf(),
        directory: name.to_owned(),
        feet_wow,
        facing_deg,
        borrows: borrows.to_owned(),
    })
}

fn map_id(name: &str) -> u32 {
    let fnv1a = name.bytes().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u32::from(byte.to_ascii_lowercase())).wrapping_mul(FNV_PRIME)
    });
    fnv1a | PAST_EVERY_MAP_DBC_ID
}

pub(crate) fn light_over_most_of(
    install: &Install,
    areas: &Areas,
    zone: u32,
) -> Result<Option<u32>, String> {
    let chain = &install.0;
    let map = CurrentMap::find(chain, &areas.get(zone).map_or(0, |a| a.map).to_string())?;
    let lights = LightCatalog::load(chain).map_err(|e| e.to_string())?;
    let world_map = WorldMapArea::read(install, zone)?;
    let tiles = world_map.map_or_else(every_tile, WorldMapArea::tiles);
    let (mut dry, mut under_ocean) = (BTreeMap::new(), BTreeMap::new());
    for (tx, ty) in tiles {
        let adt = format!("World\\Maps\\{0}\\{0}_{tx}_{ty}.adt", map.directory);
        let Some(tile) = chain.read(&adt).ok().and_then(|b| adt::parse_adt(&b).ok()) else {
            continue;
        };
        for header in tile.mcnk_chunks.iter().map(|c| &c.header) {
            let [north, west, z] = header.position;
            let middle = [north - CHUNK_SIZE / 2.0, west - CHUNK_SIZE / 2.0, z];
            let on_the_map = world_map.is_none_or(|w| w.holds(middle));
            if areas.top_zone(header.area_id) != Some(zone) || !on_the_map {
                continue;
            }
            let counted = if header.flags & MCNK_OCEAN == 0 {
                &mut dry
            } else {
                &mut under_ocean
            };
            if let Some(light) = lights.light_at(map.id, middle) {
                *counted.entry(light).or_insert(0_u32) += 1;
            }
        }
    }
    let counted = if dry.is_empty() { under_ocean } else { dry };
    Ok(counted
        .into_iter()
        .max_by_key(|&(light, n)| (n, Reverse(light)))
        .map(|(light, _)| light))
}

fn every_tile() -> Vec<(u32, u32)> {
    (0..TILES_A_SIDE)
        .flat_map(|x| (0..TILES_A_SIDE).map(move |y| (x, y)))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct WorldMapArea {
    west: f32,
    east: f32,
    north: f32,
    south: f32,
}

impl WorldMapArea {
    fn read(install: &Install, zone: u32) -> Result<Option<Self>, String> {
        let bytes = install
            .0
            .read(WORLD_MAP_AREA)
            .map_err(|e| format!("reading {WORLD_MAP_AREA}: {e}"))?;
        let mut schema = Schema::new("WorldMapArea");
        for ty in [FieldType::UInt32; 3] {
            schema.add_field(SchemaField::new("", ty));
        }
        schema.add_field(SchemaField::new("", FieldType::String));
        for _ in 0..4 {
            schema.add_field(SchemaField::new("", FieldType::Float32));
        }
        let rows = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
            .and_then(|p| p.with_schema(schema))
            .and_then(|p| p.parse_records())
            .map_err(|e| format!("parsing {WORLD_MAP_AREA}: {e}"))?;
        let float = |r: &Record, i: usize| match r.get_value(i) {
            Some(Value::Float32(v)) => Some(*v),
            _ => None,
        };
        Ok(rows
            .records()
            .iter()
            .filter(|r| matches!(r.get_value(2), Some(Value::UInt32(area)) if *area == zone))
            .find_map(|r| {
                Some(Self {
                    west: float(r, 4)?,
                    east: float(r, 5)?,
                    north: float(r, 6)?,
                    south: float(r, 7)?,
                })
            }))
    }

    fn holds(self, [x, y, _]: [f32; 3]) -> bool {
        (self.south..=self.north).contains(&x) && (self.east..=self.west).contains(&y)
    }

    fn tiles(self) -> Vec<(u32, u32)> {
        let (x0, y0) = wdt::world_to_tile(self.north, self.west);
        let (x1, y1) = wdt::world_to_tile(self.south, self.east);
        (x0..=x1)
            .flat_map(|x| (y0..=y1).map(move |y| (x, y)))
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests;
