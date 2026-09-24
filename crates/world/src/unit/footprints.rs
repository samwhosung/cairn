//! Footprints: the print a unit's foot leaves where it plants on snow or sand, projected onto the
//! ground once and faded out over six seconds.

use std::collections::{HashMap, HashSet, VecDeque};

use bevy::prelude::*;
use mpq::Chain;

use super::ViewerUnit;
use super::body::UnitBody;
use super::look::CharacterTables;
use crate::dbc_table::{read_table, str_at, u32_at};
use crate::decal::{DecalFrame, WorldDecal};
use crate::effects::{EffectVertex, WorldEffectDraw};
use crate::interior::UnitRoom;
use crate::rig_events::AnimEvent;
use crate::sky_order::{DECAL_RASTER, FOOTPRINT_SORT_RUNG};
use crate::source::{Repeat, texture_url};
use crate::surface::{SurfaceUnderfoot, Underfoot};
use crate::view::WorldCamera;

const LIFETIME: f32 = 6.0;
const OWN_CAP: usize = 64;
const SHARED_CAP: usize = 512;
/// A print reaches this far above and below the foot: to the ground under a lifted foot bone and
/// down a step, not onto a terrace below.
const SLAB_HALF_HEIGHT: f32 = 1.0;
const FOOTFALL_REACH: f32 = 50.0;
/// `TerrainType.Flags`: the ground takes prints.
const TAKES_PRINTS: u32 = 0x1;

struct Print {
    verts: Vec<EffectVertex>,
    spawned: f32,
    ink: AssetId<Image>,
    anchor: Vec3,
}

/// The live prints, the viewer's own and everyone else's in pools of their own, each oldest first:
/// all live equally long, so the oldest dies first.
#[derive(Resource, Default)]
pub(crate) struct Footprints {
    own: VecDeque<Print>,
    shared: VecDeque<Print>,
}

impl Footprints {
    fn add(&mut self, own: bool, print: Print) {
        let (pool, cap) = if own {
            (&mut self.own, OWN_CAP)
        } else {
            (&mut self.shared, SHARED_CAP)
        };
        if pool.len() >= cap {
            pool.pop_front();
        }
        pool.push_back(print);
    }

    fn retire(&mut self, now: f32) {
        for pool in [&mut self.own, &mut self.shared] {
            while pool.front().is_some_and(|p| now - p.spawned >= LIFETIME) {
                pool.pop_front();
            }
        }
    }
}

#[derive(Resource)]
pub(crate) struct FootprintTables {
    ink: HashMap<u32, Handle<Image>>,
    /// `GroundEffectTexture` id to its `TerrainType`.
    effect_terrain: HashMap<u32, u32>,
    printing: HashSet<u32>,
}

impl FootprintTables {
    fn read(chain: &Chain, mut load: impl FnMut(String) -> Handle<Image>) -> Result<Self, String> {
        let rs = read_table(chain, "DBFilesClient\\FootprintTextures.dbc", 2, &[1])?;
        let ink = rs
            .records()
            .iter()
            .filter_map(|r| {
                let url = texture_url(&format!("{}.blp", str_at(&rs, r, 1)), Repeat::BOTH);
                Some((u32_at(r, 0)?, load(url)))
            })
            .collect();
        let rs = read_table(chain, "DBFilesClient\\GroundEffectTexture.dbc", 7, &[])?;
        let effect_terrain = rs
            .records()
            .iter()
            .filter_map(|r| Some((u32_at(r, 0)?, u32_at(r, 6)?)))
            .collect();
        let rs = read_table(chain, "DBFilesClient\\TerrainType.dbc", 6, &[1])?;
        let printing = rs
            .records()
            .iter()
            .filter(|r| u32_at(r, 5).is_some_and(|f| f & TAKES_PRINTS != 0))
            .filter_map(|r| u32_at(r, 0))
            .collect();
        Ok(Self {
            ink,
            effect_terrain,
            printing,
        })
    }

    /// Whether the ground takes prints: a building's own `TerrainType`, or the one the terrain
    /// layer's ground effect names.
    fn takes_prints(&self, under: Underfoot) -> bool {
        let terrain = match under {
            Underfoot::Terrain(t) => Some(t),
            Underfoot::GroundEffect(e) => self.effect_terrain.get(&e).copied(),
        };
        terrain.is_some_and(|t| self.printing.contains(&t))
    }
}

pub(crate) fn load_tables(
    mut commands: Commands<'_, '_>,
    install: Res<'_, crate::Install>,
    server: Res<'_, AssetServer>,
) {
    match FootprintTables::read(&install.0, |url| server.load(url)) {
        Ok(tables) => {
            commands.insert_resource(tables);
        }
        Err(e) => warn!("no footprint tables, so no footprints: {e}"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Foot {
    Left,
    Right,
}

fn planted_foot(ident: [u8; 4]) -> Option<Foot> {
    let foot = match &ident[..3] {
        b"$FL" | b"$RL" | b"$SL" | b"$BL" | b"$WL" => Foot::Left,
        b"$FR" | b"$RR" | b"$SR" | b"$BR" | b"$WR" => Foot::Right,
        _ => return None,
    };
    Some(foot)
}

/// Whether a foot planted at `foot` is out of the eye's reach. The distance is squared exactly and
/// rounded once, as the client does, so a foot on the edge falls on the client's side of it.
fn footfall_culls(eye: Vec3, foot: Vec3) -> bool {
    let d = |p: f32, q: f32| f64::from(p) - f64::from(q);
    let (dx, dy, dz) = (d(eye.x, foot.x), d(eye.y, foot.y), d(eye.z, foot.z));
    (dx * dx + dy * dy + dz * dz) as f32 > FOOTFALL_REACH * FOOTFALL_REACH
}

/// The client's fade, in whole bytes: 127 from the start, then down a line to nothing at six
/// seconds.
fn fade(age: f32) -> f32 {
    let t = 1.0 - age / LIFETIME;
    if t <= 0.0 {
        return 0.0;
    }
    (255.0 * t).floor().min(127.0) / 255.0
}

/// The ink is a left foot, mirrored for a right one.
fn project_print(
    decals: &WorldDecal<'_, '_>,
    at: Vec3,
    yaw: f32,
    half: Vec2,
    foot: Foot,
) -> Option<Vec<EffectVertex>> {
    let frame = DecalFrame {
        center: at,
        turn: Rot2::radians(yaw),
        min_x: -half.x,
        max_x: half.x,
        min_z: -half.y,
        max_z: half.y,
        min_y: -SLAB_HALF_HEIGHT,
        max_y: SLAB_HALF_HEIGHT,
    };
    let mut verts = Vec::new();
    decals
        .project(
            &mut verts,
            &frame,
            |_| 1.0,
            |x, z| {
                let [u, v] = frame.rect_uv(x, z);
                [if foot == Foot::Right { 1.0 - u } else { u }, v]
            },
        )
        .then_some(verts)
}

type Walkers<'w, 's> = Query<
    'w,
    's,
    (
        &'static UnitBody,
        &'static GlobalTransform,
        Option<&'static UnitRoom>,
        Has<ViewerUnit>,
    ),
>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_footprints(
    mut events: MessageReader<'_, '_, AnimEvent>,
    time: Res<'_, Time>,
    walkers: Walkers<'_, '_>,
    camera: Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    creatures: Option<Res<'_, CharacterTables>>,
    tables: Option<Res<'_, FootprintTables>>,
    surface: SurfaceUnderfoot<'_, '_>,
    decals: WorldDecal<'_, '_>,
    mut prints: ResMut<'_, Footprints>,
) {
    let (Some(creatures), Some(tables), Ok(eye)) = (creatures, tables, camera.single()) else {
        events.clear();
        return;
    };
    let eye = eye.translation();
    for ev in events.read() {
        let Some(foot) = planted_foot(ev.ident) else {
            continue;
        };
        let Ok((body, at, room, own)) = walkers.get(ev.entity) else {
            continue;
        };
        let Some(print) = creatures.creatures.footprint(body.display) else {
            continue;
        };
        let Some(ink) = tables.ink.get(&print.texture) else {
            continue;
        };
        if footfall_culls(eye, ev.pos) {
            continue;
        }
        // The client asks the ground once for the unit, the surface its footsteps sound on, not
        // under each foot.
        let under = surface.at(room.and_then(UnitRoom::room), at.translation());
        if !under.is_some_and(|u| tables.takes_prints(u)) {
            continue;
        }
        let (scale, rotation, _) = at.to_scale_rotation_translation();
        let yaw = rotation.to_euler(EulerRot::YXZ).0;
        let half = Vec2::new(print.width, print.length) * scale.x.max(0.0) * 0.5;
        if half.min_element() <= 0.0 {
            continue;
        }
        let Some(verts) = project_print(&decals, ev.pos, yaw, half, foot) else {
            continue;
        };
        prints.add(
            own,
            Print {
                verts,
                spawned: time.elapsed_secs(),
                ink: ink.id(),
                anchor: ev.pos,
            },
        );
    }
}

pub(crate) fn push_footprints(
    time: Res<'_, Time>,
    camera: Query<'_, '_, Entity, With<WorldCamera>>,
    mut draw: WorldEffectDraw<'_>,
    mut prints: ResMut<'_, Footprints>,
) {
    let now = time.elapsed_secs();
    prints.retire(now);
    let Ok(cam) = camera.single() else {
        return;
    };
    for print in prints.own.iter().chain(&prints.shared) {
        let alpha = fade(now - print.spawned);
        let mut batch = draw
            .batch(cam, print.ink)
            .anchored(print.anchor)
            .rung(FOOTPRINT_SORT_RUNG, DECAL_RASTER);
        batch.extend(print.verts.iter().map(|v| EffectVertex {
            color: [v.color[0], v.color[1], v.color[2], v.color[3] * alpha],
            ..*v
        }));
        batch.tris();
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests;
