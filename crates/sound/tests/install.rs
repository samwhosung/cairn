use std::path::PathBuf;
use std::sync::OnceLock;

use sound::tables::{
    AreaSounds, CreatureVoices, Footsteps, KitCatalog, SoundProviders, WaterSounds,
};

fn chain() -> Option<&'static mpq::Chain> {
    static CHAIN: OnceLock<Option<mpq::Chain>> = OnceLock::new();
    CHAIN
        .get_or_init(|| {
            let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
                eprintln!("skipped: WOW_DATA is not set");
                return None;
            };
            Some(mpq::Chain::open(data).expect("open the chain"))
        })
        .as_ref()
}

#[test]
fn every_kit_loads_and_its_files_resolve_as_the_clients_do() {
    let Some(chain) = chain() else { return };
    let kits = KitCatalog::load(chain).expect("SoundEntries");
    assert_eq!(kits.len(), 4623);
    let kit = kits.get(3).expect("kit 3");
    assert_eq!(kit.name, "Invisibility Impact");
    assert_eq!(
        kit.files,
        [("Sound\\Spells\\Dispel_Low_Base.wav".to_owned(), 1)]
    );
    assert_eq!(
        (kit.min_distance, kit.distance_cutoff, kit.eax_def),
        (8.0, 45.0, 2)
    );
    assert_eq!(kits.by_name("IGMINIMAPZOOMIN").map(|k| k.id), Some(823));
    let mut eax = [0usize; 3];
    for k in kits.iter() {
        eax[k.eax_def as usize] += 1;
    }
    assert_eq!(eax, [2072, 2, 2549]);
    let (mut total, mut dead) = (0, Vec::new());
    for k in kits.iter() {
        for (path, _) in &k.files {
            total += 1;
            if !chain.contains(path) {
                dead.push(k.id);
            }
        }
    }
    assert_eq!((total, total - dead.len()), (8961, 8934));
    assert_eq!(dead.iter().filter(|&&id| id == 8940).count(), 10);
}

#[test]
fn elwynn_and_the_abbey_resolve_their_music_ambience_and_reverb() {
    let Some(chain) = chain() else { return };
    let areas = AreaSounds::load(chain).expect("the area tables");
    assert_eq!(areas.len(), 1081);
    let elwynn = areas.resolve(12).expect("Elwynn Forest");
    assert_eq!(elwynn.name, "Elwynn Forest");
    assert_eq!(elwynn.sound_provider, [0, 11]);
    let music = elwynn.music.expect("zone music");
    assert_eq!(music.set_name, "Zone-Forest");
    assert_eq!(
        (music.silence_min, music.silence_max),
        ([180_000; 2], [300_000; 2])
    );
    assert_eq!(music.sounds, [2523, 2523]);
    assert!(elwynn.ambience.is_some());
    assert_eq!(areas.resolve(24).map(|a| a.sound_provider), Some([73, 11]));
    let providers = SoundProviders::load(chain).expect("the reverb presets");
    assert_eq!(providers.len(), 38);
    let generic = providers.get(67).expect("PRESET_GENERIC");
    assert_eq!(
        (generic.room, generic.room_hf, generic.reverb),
        (-1000, -100, 200)
    );
    assert_eq!(providers.get(11).map(|p| p.room_hf), Some(-10000));
}

#[test]
fn the_footstep_chain_and_the_liquid_loops_resolve() {
    let Some(chain) = chain() else { return };
    let steps = Footsteps::load(chain).expect("the footstep tables");
    assert_eq!(steps.len(), 179);
    let voices = CreatureVoices::load(chain).expect("the creature voices");
    assert_eq!(
        voices.footstep_class(49),
        Some(7),
        "a human male walks as class 7"
    );
    assert_eq!(voices.footstep_class(26), Some(8));
    let water = WaterSounds::load(chain).expect("SoundWaterType");
    assert_eq!(water.len(), 12);
    let loops: Vec<Option<u32>> = [0, 4, 8, 1, 5, 2, 6, 3, 7, 0xf]
        .map(|n| water.kit_for_nibble(n))
        .to_vec();
    let want = [1111, 1112, 1113, 1114, 1114, 3072, 3052, 3880, 3880];
    assert_eq!(&loops[..9], want.map(Some));
    assert_eq!(loops[9], None);
}
