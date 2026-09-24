//! The collision half of the world's files, as assets of their own. Typed loads pick these loaders
//! over the render ones that share the extensions.

use std::io;

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::math::Vec3;
use bevy::reflect::TypePath;
use model::{CollisionMesh, WmoDoodad, WmoDoodadSet};
use terrain::{ChunkMesh, Doodad, LiquidMesh, WmoInstance};

use super::colliders::{impassable_wall_data, terrain_collider_data};
use crate::source::MPQ_SOURCE;
use crate::wmo::{RoomsBuilder, WmoRooms};

type Soup = (Vec<Vec3>, Vec<[u32; 3]>);

/// One ADT tile's collision: the ground, the impassable fences, the liquid surfaces and what the
/// tile places.
#[derive(Asset, TypePath)]
pub struct TileCollision {
    pub(super) terrain: Option<Soup>,
    pub(super) walls: Option<Soup>,
    pub(super) liquids: Vec<LiquidMesh>,
    pub(super) doodads: Vec<Doodad>,
    pub(super) wmos: Vec<WmoInstance>,
    pub(super) ground: Vec<ChunkMesh>,
}

/// An M2's collision hull in model space; `None` when the model has none.
#[derive(Asset, TypePath)]
pub struct M2Hull(pub Option<CollisionMesh>);

/// A WMO's walking faces and camera faces over all its groups, in model space, and the doodads
/// its sets place.
#[derive(Asset, TypePath)]
pub struct WmoHull {
    pub(super) walk: Option<CollisionMesh>,
    pub(super) camera: Option<CollisionMesh>,
    pub(super) doodads: Vec<WmoDoodad>,
    pub(super) doodad_sets: Vec<WmoDoodadSet>,
    pub(super) rooms: WmoRooms,
}

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

async fn read_all(reader: &mut dyn Reader) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;
    Ok(bytes)
}

#[derive(TypePath)]
pub(super) struct TileCollisionLoader;

impl AssetLoader for TileCollisionLoader {
    type Asset = TileCollision;
    type Settings = ();
    type Error = io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        _ctx: &mut LoadContext<'_>,
    ) -> io::Result<TileCollision> {
        let mut tile = terrain::adt_to_tile_mesh(&read_all(reader).await?).map_err(invalid)?;
        let liquids = tile
            .chunks
            .iter_mut()
            .flat_map(|c| std::mem::take(&mut c.liquids))
            .collect();
        for chunk in &mut tile.chunks {
            chunk.alpha_map = None;
        }
        Ok(TileCollision {
            terrain: terrain_collider_data(&tile.chunks),
            walls: impassable_wall_data(&tile.chunks),
            liquids,
            doodads: tile.doodads,
            wmos: tile.wmos,
            ground: tile.chunks,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["adt"]
    }
}

#[derive(TypePath)]
pub(super) struct M2HullLoader;

impl AssetLoader for M2HullLoader {
    type Asset = M2Hull;
    type Settings = ();
    type Error = io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        _ctx: &mut LoadContext<'_>,
    ) -> io::Result<M2Hull> {
        let hull = model::parse_m2_collision_hull(&read_all(reader).await?).map_err(invalid)?;
        Ok(M2Hull((!hull.is_empty()).then_some(hull)))
    }

    fn extensions(&self) -> &[&str] {
        &["m2"]
    }
}

#[derive(TypePath)]
pub(super) struct WmoHullLoader;

impl AssetLoader for WmoHullLoader {
    type Asset = WmoHull;
    type Settings = ();
    type Error = io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        ctx: &mut LoadContext<'_>,
    ) -> io::Result<WmoHull> {
        let bytes = read_all(reader).await?;
        let root = model::parse_wmo_root(&bytes).map_err(invalid)?;
        let path = ctx.path().path().to_string_lossy().to_ascii_lowercase();
        let stem = path.strip_suffix(".wmo").unwrap_or(&path).to_owned();
        let (mut walk, mut camera) = (CollisionMesh::default(), CollisionMesh::default());
        let mut rooms = RoomsBuilder::new(&root, &bytes);
        for gi in 0..root.group_count() {
            // A group that is missing or does not parse is skipped, as the client skips it.
            let Ok(group) = ctx
                .read_asset_bytes(format!("{MPQ_SOURCE}://{stem}_{gi:03}.wmo"))
                .await
            else {
                continue;
            };
            model::accumulate_wmo_group_collision(&group, &mut walk.positions, &mut walk.indices);
            model::accumulate_wmo_group_camera_collision(
                &group,
                &mut camera.positions,
                &mut camera.indices,
            );
            rooms.add_group(gi as usize, &group);
        }
        Ok(WmoHull {
            walk: (!walk.is_empty()).then_some(walk),
            camera: (!camera.is_empty()).then_some(camera),
            doodads: root.doodads().to_vec(),
            doodad_sets: root.doodad_sets().to_vec(),
            rooms: rooms.finish(),
        })
    }

    fn extensions(&self) -> &[&str] {
        &["wmo"]
    }
}
