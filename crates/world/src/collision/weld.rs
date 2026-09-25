//! Doodad hulls welded into batches: thousands of small colliders cost avian per-collider work
//! every frame, so hulls are concatenated into a few dozen trimeshes, as the terrain's chunks
//! are. Map doodads weld under the tile that first placed them and go with that tile; a WMO's
//! props weld under the building and go with it.

use std::collections::BTreeMap;

use bevy::prelude::*;

use super::colliders::{PendingCollider, build_collider_task};
use super::stream::CollisionStreamer;

const WELD_MAX_HULLS: u32 = 128;
/// A single oversized hull may exceed this alone; it then closes its batch at once.
const WELD_MAX_TRIS: usize = 16_384;
/// Quiet frames that close a batch still taking hulls.
const WELD_IDLE_FRAMES: u32 = 15;

struct WeldAcc {
    verts: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
    hulls: u32,
    last_add: u32,
}

impl WeldAcc {
    fn new(frame: u32) -> Self {
        Self {
            verts: Vec::new(),
            tris: Vec::new(),
            hulls: 0,
            last_add: frame,
        }
    }

    fn append(&mut self, verts: Vec<Vec3>, tris: Vec<[u32; 3]>, frame: u32) {
        let base = self.verts.len() as u32;
        self.verts.extend(verts);
        self.tris
            .extend(tris.into_iter().map(|t| t.map(|i| base + i)));
        self.hulls += 1;
        self.last_add = frame;
    }

    fn ready(&self, frame: u32) -> bool {
        self.hulls >= WELD_MAX_HULLS
            || self.tris.len() >= WELD_MAX_TRIS
            || frame.wrapping_sub(self.last_add) >= WELD_IDLE_FRAMES
    }

    fn spawn(&mut self, commands: &mut Commands<'_, '_>) -> Entity {
        let verts = std::mem::take(&mut self.verts);
        let tris = std::mem::take(&mut self.tris);
        commands
            .spawn((
                Transform::IDENTITY,
                PendingCollider::new(build_collider_task(verts, tris), None),
            ))
            .id()
    }
}

#[derive(Default)]
pub(super) struct HullWelds {
    frame: u32,
    tiles: BTreeMap<(u32, u32), WeldAcc>,
    props: BTreeMap<u32, WeldAcc>,
}

impl HullWelds {
    pub(super) fn add_tile(&mut self, tile: (u32, u32), verts: Vec<Vec3>, tris: Vec<[u32; 3]>) {
        let frame = self.frame;
        self.tiles
            .entry(tile)
            .or_insert_with(|| WeldAcc::new(frame))
            .append(verts, tris, frame);
    }

    pub(super) fn add_prop(&mut self, placement: u32, verts: Vec<Vec3>, tris: Vec<[u32; 3]>) {
        let frame = self.frame;
        self.props
            .entry(placement)
            .or_insert_with(|| WeldAcc::new(frame))
            .append(verts, tris, frame);
    }

    /// Batches not yet handed to the build queue.
    pub(super) fn unflushed(&self) -> usize {
        self.tiles.len() + self.props.len()
    }
}

/// Closes ready batches into colliders owned by their tile or building; a batch whose owner has
/// streamed out is dropped with it.
pub(super) fn flush_welds(
    mut commands: Commands<'_, '_>,
    mut streamer: ResMut<'_, CollisionStreamer>,
) {
    let streamer = &mut *streamer;
    let welds = &mut streamer.welds;
    welds.frame = welds.frame.wrapping_add(1);
    let frame = welds.frame;
    welds.tiles.retain(|key, acc| {
        let Some(tile) = streamer.tiles.get_mut(key) else {
            return false;
        };
        if !acc.ready(frame) {
            return true;
        }
        tile.entities.push(acc.spawn(&mut commands));
        false
    });
    welds.props.retain(|uid, acc| {
        let Some(p) = streamer.placements.get_mut(uid) else {
            return false;
        };
        if !acc.ready(frame) {
            return true;
        }
        p.entities.push(acc.spawn(&mut commands));
        false
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hull() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        (vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]])
    }

    #[test]
    fn a_batch_rebases_each_hulls_indices_and_closes_at_its_caps() {
        let mut acc = WeldAcc::new(0);
        for _ in 0..3 {
            let (v, t) = hull();
            acc.append(v, t, 0);
        }
        assert_eq!(acc.tris, vec![[0, 1, 2], [3, 4, 5], [6, 7, 8]]);
        assert!(!acc.ready(0), "a fresh batch waits for more");
        assert!(acc.ready(WELD_IDLE_FRAMES), "and closes once quiet");
        for _ in 3..WELD_MAX_HULLS {
            let (v, t) = hull();
            acc.append(v, t, 0);
        }
        assert!(acc.ready(0), "the hull cap closes it at once");
    }
}
