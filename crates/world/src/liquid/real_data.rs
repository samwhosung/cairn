//! The liquid query against the client's own files, at the places where a rule once answered with
//! the wrong surface. The surfaces are built the way the world builds them and asked what the
//! world asks. Every test skips without `WOW_DATA`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use bevy::prelude::*;
use light::Submersion;
use mpq::Chain;
use terrain::LiquidKind;

use super::query::{
    LiquidClaim, LiquidGrid, LiquidSource, WmoPool, liquid_at, submersion_claim_at, wet_footprint,
};
use super::surface::scrolls;
use crate::coords::{bevy_to_wow, placement_rotation, wow_to_bevy};
use crate::interior::WmoRoom;
use crate::wmo::{RoomsBuilder, WmoGroupNav, WmoRooms};

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(&data).expect("open the chain"))
}

/// A building's rooms from its root and group files, as the world loads them.
fn rooms(chain: &Chain, root_path: &str) -> Option<WmoRooms> {
    let bytes = chain.read(root_path).ok()?;
    let root = model::parse_wmo_root(&bytes).ok()?;
    let lower = root_path.to_ascii_lowercase();
    let stem = lower.strip_suffix(".wmo").unwrap_or(&lower);
    let mut builder = RoomsBuilder::new(&root, &bytes);
    for gi in 0..root.group_count() as usize {
        if let Ok(group) = chain.read(&format!("{stem}_{gi:03}.wmo")) {
            builder.add_group(gi, &group);
        }
    }
    Some(builder.finish())
}

struct Placement {
    instance: Entity,
    model: String,
    transform: Transform,
    nav: Vec<WmoGroupNav>,
}

/// Every liquid surface of the tiles around a point: the terrain's, and each placed building's
/// owned by a stand-in for the entity the world spawns for the placement.
struct LiquidScene {
    surfaces: Vec<LiquidGrid>,
    placements: Vec<Placement>,
}

impl LiquidScene {
    /// The first placement one of whose group boxes holds `wow`, and the claim of standing in
    /// that group. Coarser than the world's down-ray, which also races the faces and the ground,
    /// and enough to say which building a point is in.
    fn claim_at(&self, wow: [f32; 3]) -> Option<(LiquidClaim, &str)> {
        self.placements.iter().find_map(|p| {
            let local = bevy_to_wow(
                p.transform
                    .compute_affine()
                    .inverse()
                    .transform_point3(wow_to_bevy(wow)),
            );
            let group = p.nav.iter().position(|g| {
                (0..3).all(|i| local[i] >= g.bbox_min[i] && local[i] <= g.bbox_max[i])
            })?;
            let room = WmoRoom {
                instance: p.instance,
                group: group as u16,
            };
            Some((LiquidClaim::inside(room, &p.nav), p.model.as_str()))
        })
    }

    /// The surface the query answers for `claim`, and the highest wet vertex of every surface
    /// over the column: the rule the grid replaced.
    fn verdict(&self, wow: [f32; 3], claim: LiquidClaim) -> Option<(f32, f32)> {
        let hit = liquid_at(self.surfaces.iter(), wow, claim)?;
        let highest = self
            .surfaces
            .iter()
            .filter(|g| g.surface_z_at(wow[0], wow[1]).is_some())
            .map(LiquidGrid::highest_wet_z)
            .fold(f32::MIN, f32::max);
        Some((hit.surface_z, highest))
    }
}

fn liquid_scene(map: &str, wow: [f32; 3]) -> Option<LiquidScene> {
    let chain = chain_or_skip()?;
    let (tx, ty) = wdt::world_to_tile(wow[0], wow[1]);
    let mut scene = LiquidScene {
        surfaces: Vec::new(),
        placements: Vec::new(),
    };
    let mut placed = HashSet::new();
    // A building as big as Blackrock is placed from every tile it overlaps.
    for (dx, dy) in (-1..=1).flat_map(|dx| (-1..=1).map(move |dy| (dx, dy))) {
        let (Some(x), Some(y)) = (tx.checked_add_signed(dx), ty.checked_add_signed(dy)) else {
            continue;
        };
        let Ok(tile) = terrain::load_tile_mesh(&chain, map, x, y) else {
            continue;
        };
        for lq in tile.chunks.iter().flat_map(|c| &c.liquids) {
            let grid = wet_footprint(lq, &Transform::IDENTITY, LiquidSource::AdtChunk);
            scene.surfaces.push(grid);
        }
        for w in &tile.wmos {
            if !placed.insert(w.unique_id) {
                continue;
            }
            let Some(rooms) = rooms(&chain, &w.model) else {
                continue;
            };
            let transform = Transform {
                translation: wow_to_bevy(w.position),
                rotation: placement_rotation(w.rotation),
                scale: Vec3::ONE,
            };
            let index = u32::try_from(scene.placements.len()).expect("few placements");
            let instance = Entity::from_raw_u32(index).expect("a valid index");
            for (gi, lq) in rooms.group_liquids.iter().enumerate() {
                let Some(lq) = lq else { continue };
                let pool = WmoPool::of(&rooms, gi, instance, &transform);
                let grid = wet_footprint(lq, &transform, LiquidSource::WmoGroup(pool));
                scene.surfaces.push(grid);
            }
            scene.placements.push(Placement {
                instance,
                model: w.model.to_ascii_lowercase(),
                transform,
                nav: rooms.group_nav,
            });
        }
    }
    (!scene.surfaces.is_empty()).then_some(scene)
}

/// Blackrock Mountain's lava under its stairs (`blackrock.wmo` group 38, a 55 × 82 magma grid
/// falling from 175.00 to 167.29 under a turned placement): the highest wet vertex stood 2.4
/// yards over the feet, past the swim line, with the lava yards below.
#[test]
fn blackrock_lava_is_below_the_feet_not_above_it() {
    let feet = [-7531.21_f32, -1123.64, 172.58];
    let Some(scene) = liquid_scene("Azeroth", feet) else {
        return;
    };
    let claim = scene
        .claim_at(feet)
        .map_or(LiquidClaim::Unknown, |(claim, _)| claim);
    let (surface, highest) = scene.verdict(feet, claim).expect("the lava answers");
    assert!((highest - 175.00).abs() < 0.05, "{highest}");
    assert!((surface - 168.45).abs() < 0.05, "{surface}");
    assert!(surface < feet[2], "{surface} under the feet");
}

/// Felfire Hill's river, one chunk falling from 99.56 to 95.78: on its bank the water is at the
/// soles, where the highest wet vertex put it 1.56 yards over them.
#[test]
fn felfire_hill_river_does_not_swim_on_the_bank() {
    let feet = [1983.97_f32, -2875.84, 98.00];
    let Some(scene) = liquid_scene("Kalimdor", feet) else {
        return;
    };
    let (surface, highest) = scene
        .verdict(feet, LiquidClaim::Outdoors)
        .expect("the river answers");
    assert!((highest - 99.56).abs() < 0.05, "{highest}");
    assert!(surface < feet[2] && feet[2] - surface < 1.0, "{surface}");
}

/// In Uldaman, a nearby mushroom cave's pool (`md_mushroomcave.wmo` group 1) covers the column
/// 186 yards overhead. Its room's floor keeps it from anyone below, and it is another building's.
#[test]
fn uldaman_is_not_submerged_in_a_mushroom_caves_pool() {
    let feet = [-6152.73_f32, -2969.59, 213.73];
    let Some(scene) = liquid_scene("Azeroth", feet) else {
        return;
    };
    let overhead = scene
        .surfaces
        .iter()
        .filter_map(|g| g.surface_z_at(feet[0], feet[1]))
        .filter(|&z| z > feet[2])
        .min_by(f32::total_cmp)
        .expect("the cave's pool is over the column");
    assert!((overhead - 399.64).abs() < 0.05, "{overhead}");
    assert!(
        liquid_at(scene.surfaces.iter(), feet, LiquidClaim::Unknown).is_none(),
        "the pool's floor alone keeps it off a subject 186 yards under it"
    );
    let (claim, model) = scene.claim_at(feet).expect("in a building");
    assert!(model.contains("uldaman"), "{model}");
    assert!(liquid_at(scene.surfaces.iter(), feet, claim).is_none());
}

/// Undercity's upper channels (groups 7 and 10, slime at 51.98) cover a room 115 yards below
/// whose own slime lies at −64.48, under the eye. The same building owns both, so only the
/// floor keeps the channels off the room.
#[test]
fn undercitys_upper_channels_do_not_submerge_the_rooms_below() {
    let eye = [1732.68_f32, 187.01, -63.59];
    let Some(scene) = liquid_scene("Azeroth", eye) else {
        return;
    };
    let overhead: Vec<f32> = scene
        .surfaces
        .iter()
        .filter_map(|g| g.surface_z_at(eye[0], eye[1]))
        .filter(|&z| z > eye[2])
        .collect();
    assert!(
        !overhead.is_empty() && overhead.iter().all(|z| (z - 51.98).abs() < 0.05),
        "{overhead:?}"
    );
    let (claim, model) = scene.claim_at(eye).expect("in a building");
    assert!(model.contains("undercity"), "{model}");
    let submersion = |at: [f32; 3]| {
        submersion_claim_at(scene.surfaces.iter(), at, claim).map_or(Submersion::Dry, |(s, _)| s)
    };
    assert_eq!(submersion(eye), Submersion::Dry);
    let in_the_slime = [eye[0], eye[1], -66.0];
    assert_eq!(submersion(in_the_slime), Submersion::Slime);
    let hit = liquid_at(scene.surfaces.iter(), in_the_slime, claim).expect("the room's slime");
    assert!((hit.surface_z + 64.48).abs() < 0.05, "{}", hit.surface_z);
}

/// The Rogues' Quarter is cut into the rock 95 yards under Tirisfal's lake, and no pool of
/// Undercity's covers it.
#[test]
fn the_rogues_quarter_is_not_under_tirisfals_lake() {
    let feet = [1414.08_f32, 53.00, -62.26];
    let Some(scene) = liquid_scene("Azeroth", feet) else {
        return;
    };
    let lake = liquid_at(scene.surfaces.iter(), feet, LiquidClaim::Unknown)
        .expect("the lake covers the column");
    assert!((lake.surface_z - 32.93).abs() < 0.05, "{}", lake.surface_z);
    let (claim, model) = scene.claim_at(feet).expect("in a building");
    assert!(model.contains("undercity"), "{model}");
    assert!(liquid_at(scene.surfaces.iter(), feet, claim).is_none());
}

/// The Felfire channel falls about a tenth of a yard per yard over 60 yards: the steepest run the
/// swim latch's hysteresis must hold along.
#[test]
fn the_felfire_channel_falls_about_a_tenth_of_a_yard_per_yard() {
    let (downstream, upstream) = ([1953.97_f32, -2866.84, 0.0], [2013.97_f32, -2866.84, 0.0]);
    let Some(scene) = liquid_scene("Kalimdor", downstream) else {
        return;
    };
    let z = |at: [f32; 3]| {
        liquid_at(scene.surfaces.iter(), at, LiquidClaim::Outdoors)
            .unwrap_or_else(|| panic!("no river at {at:?}"))
            .surface_z
    };
    let slope = (z(upstream) - z(downstream)) / (upstream[0] - downstream[0]);
    assert!((slope - 0.099).abs() < 0.005, "{slope}");
}

/// A building's pools by their type nibble and kind.
fn pool_census(rooms: &WmoRooms) -> HashMap<(u8, LiquidKind), usize> {
    let mut census = HashMap::new();
    for lq in rooms.group_liquids.iter().flatten() {
        *census.entry((lq.sound_nibble, lq.kind)).or_default() += 1;
    }
    census
}

/// Magma and slime scroll by the pool, not by the kind: Ironforge holds both still and moving
/// lava, Undercity's slime stands while Stratholme's flows.
#[test]
fn only_the_nibble_six_and_seven_pools_scroll_in_the_shipped_data() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let census = |path: &str| pool_census(&rooms(&chain, path).expect("the building loads"));
    let ironforge = census("world\\wmo\\khazmodan\\cities\\ironforge\\ironforge.wmo");
    assert_eq!(
        ironforge,
        HashMap::from([
            ((2, LiquidKind::Magma), 9),
            ((4, LiquidKind::Still), 2),
            ((6, LiquidKind::Magma), 2),
        ])
    );
    let blackrock = census("world\\wmo\\dungeon\\az_blackrock\\blackrock.wmo");
    assert_eq!(blackrock, HashMap::from([((6, LiquidKind::Magma), 2)]));
    let undercity = census("world\\wmo\\lorderon\\undercity\\undercity.wmo");
    assert_eq!(undercity, HashMap::from([((3, LiquidKind::Slime), 38)]));
    let stratholme = census("world\\wmo\\dungeon\\ld_stratholme\\stratholme.wmo");
    assert_eq!(stratholme, HashMap::from([((7, LiquidKind::Slime), 3)]));
    let scrolling = |c: &HashMap<(u8, LiquidKind), usize>| -> usize {
        c.iter()
            .filter(|((n, _), _)| scrolls(*n))
            .map(|(_, count)| count)
            .sum()
    };
    assert_eq!(scrolling(&ironforge), 2);
    assert_eq!(scrolling(&blackrock), 2);
    assert_eq!(scrolling(&stratholme), 3);
    assert_eq!(scrolling(&undercity), 0);
}

/// A pool fogs as indoors when its group is interior. Undercity's pools are all interior but
/// group 7; every one of Stormwind's canals and fountains is open to the sky.
#[test]
fn a_wmo_pools_fog_block_follows_its_groups_interior_class() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let pools = |path: &str| -> Vec<(usize, bool)> {
        let rooms = rooms(&chain, path).expect("the building loads");
        (0..rooms.group_liquids.len())
            .filter(|&gi| rooms.group_liquids[gi].is_some())
            .map(|gi| (gi, rooms.group_nav[gi].interior))
            .collect()
    };
    let undercity = pools("world\\wmo\\lorderon\\undercity\\undercity.wmo");
    let exterior: Vec<usize> = undercity
        .iter()
        .filter(|(_, interior)| !interior)
        .map(|&(gi, _)| gi)
        .collect();
    assert_eq!(undercity.len(), 38, "{undercity:?}");
    assert_eq!(exterior, vec![7]);
    let stormwind = pools("world\\wmo\\azeroth\\buildings\\stormwind\\stormwind.wmo");
    assert_eq!(stormwind.len(), 22, "{stormwind:?}");
    assert!(stormwind.iter().all(|(_, interior)| !interior));
}
