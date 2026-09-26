use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

const ELWYNN: &str = "\
# a valley of its own
name = Stillmere
start = -250, -260.5, 12   # the feet
facing = 90

borrows = Elwynn Forest
";

const GOLDSHIRE: [f32; 3] = [-9439.1, 71.2, 68.0];
const DUSKWOOD_MIDDLE: [f32; 3] = [-10640.0, -880.0, 50.0];
pub(crate) const ELWYNN_FOREST_AREA: u32 = 12;
pub(crate) const DUSKWOOD_AREA: u32 = 10;

pub(crate) struct TestZone(PathBuf);

impl TestZone {
    pub(crate) fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("cairn-zone-{label}-{}-{n}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("a directory for the zone");
        Self(dir)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn write_file(&self, text: &str) {
        std::fs::write(self.0.join(FILE), text).expect("the zone's file");
    }

    pub(crate) fn write_map(&self, name: &str, tiles: &[((u32, u32), Vec<u8>)]) {
        let dir = self.0.join("World").join("Maps").join(name);
        std::fs::create_dir_all(&dir).expect("the map's directory");
        let mut main = vec![0u8; 64 * 64 * 8];
        for ((x, y), bytes) in tiles {
            main[(*y as usize * 64 + *x as usize) * 8] = 1;
            std::fs::write(dir.join(format!("{name}_{x}_{y}.adt")), bytes).expect("a tile");
        }
        let mut wdt = Vec::new();
        for (magic, payload) in [
            (b"REVM", 18u32.to_le_bytes().to_vec()),
            (b"DHPM", vec![0u8; 32]),
            (b"NIAM", main),
        ] {
            wdt.extend_from_slice(magic);
            wdt.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            wdt.extend_from_slice(&payload);
        }
        std::fs::write(dir.join(format!("{name}.wdt")), wdt).expect("the WDT");
    }
}

impl Drop for TestZone {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn parsed(text: &str) -> Result<Zone, String> {
    parse(Path::new("z"), text)
}

#[test]
fn a_zone_file_names_the_map_the_start_and_the_zone_it_borrows() {
    let zone = parsed(ELWYNN).expect("parses");
    let feet_wow = Vec3::new(-250.0, -260.5, 12.0);
    assert_eq!(
        zone,
        Zone {
            root: PathBuf::from("z"),
            directory: "Stillmere".into(),
            feet_wow,
            facing_deg: 90.0,
            borrows: "Elwynn Forest".into(),
        }
    );
    assert_eq!(zone.start(), Aim::start(feet_wow, 90.0));
    let facing_north = parsed("name=A_1\nstart=1,2,3\nborrows=Duskwood").expect("parses");
    assert_eq!(
        facing_north.start(),
        Aim::start(Vec3::new(1.0, 2.0, 3.0), 0.0)
    );
}

#[test]
fn a_mistake_in_a_zone_file_is_named_with_its_line() {
    let whole = |line: &str| format!("name = A\nstart = 1, 2, 3\nborrows = Duskwood\n{line}");
    for (text, said) in [
        (
            "start = 1,2,3\nborrows = Duskwood".into(),
            "z/zone.txt names no name",
        ),
        ("name = A\nborrows = Duskwood".into(), "names no start"),
        ("name = A\nstart = 1,2,3".into(), "names no borrows"),
        (
            whole("facing = north"),
            "z/zone.txt:4: facing wants degrees",
        ),
        (whole("facing = 1, 2"), ":4: facing wants"),
        (whole("colour = red"), ":4: no key `colour`"),
        (whole("name = B"), ":4: `name` again, after line 1"),
        (whole("just words"), ":4: want `key = value`"),
        (
            "name = A\nstart = 1, 2\nborrows = D".into(),
            ":2: start wants X, Y, Z",
        ),
        (
            "name = A\nstart = 1,2,inf\nborrows = D".into(),
            ":2: start wants",
        ),
        (
            "name = A\nstart = 1,2,x\nborrows = D".into(),
            ":2: start wants",
        ),
        (
            "name = A\nstart = 1,2,3\nborrows =".into(),
            ":3: borrows wants",
        ),
    ] {
        let said_back = parsed(&text).expect_err(&text);
        assert!(said_back.contains(said), "{text:?}: {said_back}");
    }
    for name in ["1A", "A B", "A-B", "A/B", "Ä", "_A", ""] {
        let text = format!("name = {name}\nstart = 1,2,3\nborrows = D");
        let said = parsed(&text).expect_err(name);
        assert!(
            said.contains(":1: a name is its map's directory"),
            "{name}: {said}"
        );
    }
}

#[test]
fn a_zone_file_cut_short_or_with_a_bit_flipped_is_read_or_refused_and_never_panics() {
    let zone = TestZone::new("flipped");
    let bytes = ELWYNN.as_bytes();
    let mut refused = 0;
    let mut each = |bytes: &[u8]| {
        std::fs::write(zone.path().join(FILE), bytes).expect("the file");
        refused += usize::from(Zone::read(zone.path()).is_err());
    };
    for n in 0..bytes.len() {
        each(&bytes[..n]);
    }
    for bit in 0..bytes.len() * 8 {
        let mut flipped = bytes.to_vec();
        flipped[bit / 8] ^= 1 << (bit % 8);
        each(&flipped);
    }
    assert!(refused > bytes.len(), "{refused} refused");
}

#[test]
fn a_directory_without_its_file_is_refused_by_name() {
    let zone = TestZone::new("fileless");
    for dir in [zone.path().to_path_buf(), zone.path().join("absent")] {
        let said = Zone::read(&dir).expect_err("no file");
        assert!(
            said.starts_with(&format!("{} has no zone.txt", dir.display())),
            "{said}"
        );
    }
}

#[test]
fn a_zone_s_map_id_is_none_of_the_installs_and_its_names_alone() {
    assert!(map_id("Stillmere") >= PAST_EVERY_MAP_DBC_ID);
    assert_eq!(map_id("Stillmere"), map_id("STILLMERE"));
    assert_ne!(map_id("Stillmere"), map_id("Fernvale"));
}

fn opened(zone: &TestZone) -> Option<Result<CurrentMap, String>> {
    let Some(data) = std::env::var_os("WOW_DATA") else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    let install = Install::open_patched(Path::new(&data), zone.path()).expect("the install");
    Some(Zone::read(zone.path()).and_then(|z| z.open(&install)))
}

fn borrowing(borrows: &str) -> Option<Result<CurrentMap, String>> {
    let zone = TestZone::new("borrowing");
    zone.write_file(&ELWYNN.replace("Elwynn Forest", borrows));
    zone.write_map("Stillmere", &[]);
    opened(&zone)
}

#[test]
fn a_zone_is_lit_by_the_light_most_of_the_borrowed_zones_ground_stands_in() {
    let Some(elwynn) = borrowing("elwynn forest") else {
        return;
    };
    let elwynn = elwynn.expect("opens");
    let duskwood = borrowing("Duskwood").expect("the install").expect("opens");
    let data = std::env::var_os("WOW_DATA").expect("the install");
    let lights = LightCatalog::load(&mpq::Chain::open(data).expect("the install")).expect("lit");
    let light = |at| lights.light_at(0, at).expect("a light");
    assert_eq!(
        elwynn.borrowed,
        Some(Borrowed {
            light: light(GOLDSHIRE),
            area: ELWYNN_FOREST_AREA
        })
    );
    assert_eq!(
        duskwood.borrowed,
        Some(Borrowed {
            light: light(DUSKWOOD_MIDDLE),
            area: DUSKWOOD_AREA
        })
    );
    assert_ne!(light(GOLDSHIRE), light(DUSKWOOD_MIDDLE));
    assert_eq!(
        (elwynn.id, elwynn.directory.as_str()),
        (map_id("Stillmere"), "Stillmere")
    );
}

#[test]
fn a_zone_the_install_cannot_hold_is_refused_by_name() {
    let Some(unknown) = borrowing("Elwinn Forest") else {
        return;
    };
    let said = unknown.expect_err("no such zone");
    assert!(
        said.ends_with("zone.txt: the install has no zone named Elwinn Forest"),
        "{said}"
    );
    let said = borrowing("Ragefire Chasm")
        .expect("the install")
        .expect_err("no ground");
    assert!(
        said.contains("Ragefire Chasm has no ground in the install"),
        "{said}"
    );

    let taken = TestZone::new("taken");
    taken.write_file(&ELWYNN.replace("Stillmere", "Azeroth"));
    let said = opened(&taken).expect("the install").expect_err("taken");
    assert!(said.contains("Azeroth is a map of the install's"), "{said}");

    let mapless = TestZone::new("mapless");
    mapless.write_file(ELWYNN);
    let said = opened(&mapless).expect("the install").expect_err("no map");
    assert!(
        said.ends_with("has no World/Maps/Stillmere/Stillmere.wdt"),
        "{said}"
    );
}
