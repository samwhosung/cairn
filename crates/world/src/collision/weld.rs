//! Doodad hulls welded into batches: thousands of small colliders cost avian per-collider work
//! every frame, so hulls are concatenated into a few dozen trimeshes, as the terrain's chunks are.

use bevy::prelude::*;

use super::colliders::{PendingCollider, build_collider_task};

const WELD_MAX_HULLS: usize = 128;
const WELD_CLOSE_AT_TRIS: usize = 16_384;

pub(super) type Trimesh = (Vec<Vec3>, Vec<[u32; 3]>);

pub(super) fn weld(hulls: impl IntoIterator<Item = Trimesh>) -> Vec<Trimesh> {
    let mut batches = Vec::new();
    let (mut verts, mut tris, mut count): (Vec<Vec3>, Vec<[u32; 3]>, usize) = Default::default();
    for (hull_verts, hull_tris) in hulls {
        let base = verts.len() as u32;
        verts.extend(hull_verts);
        tris.extend(hull_tris.into_iter().map(|t| t.map(|i| base + i)));
        count += 1;
        if count >= WELD_MAX_HULLS || tris.len() >= WELD_CLOSE_AT_TRIS {
            batches.push((std::mem::take(&mut verts), std::mem::take(&mut tris)));
            count = 0;
        }
    }
    if count > 0 {
        batches.push((verts, tris));
    }
    batches
}

pub(super) fn spawn_batch(commands: &mut Commands<'_, '_>, (verts, tris): Trimesh) -> Entity {
    commands
        .spawn((
            Transform::IDENTITY,
            PendingCollider::new(build_collider_task(verts, tris), None),
        ))
        .id()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hull() -> Trimesh {
        (vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]])
    }

    #[test]
    fn a_batch_rebases_each_hulls_indices_and_closes_at_its_caps() {
        let three = weld((0..3).map(|_| hull()));
        assert_eq!(three.len(), 1);
        assert_eq!(three[0].1, vec![[0, 1, 2], [3, 4, 5], [6, 7, 8]]);
        let over = weld((0..=WELD_MAX_HULLS).map(|_| hull()));
        assert_eq!(over.len(), 2, "the hull cap closes a batch");
        assert_eq!(over[1].1, vec![[0, 1, 2]], "and the next starts from zero");
        let big = (vec![Vec3::ZERO; 3], vec![[0, 1, 2]; WELD_CLOSE_AT_TRIS]);
        let capped = weld([hull(), big, hull()]);
        assert_eq!(capped.len(), 2, "the triangle cap closes a batch");
        assert_eq!(capped[1].1, vec![[0, 1, 2]]);
        assert!(weld(std::iter::empty()).is_empty());
    }
}
