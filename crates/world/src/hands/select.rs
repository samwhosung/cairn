use std::collections::{BTreeMap, BTreeSet, HashMap};

use bevy::asset::{AssetId, RenderAssetUsages};
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::mesh::{MeshTag, PrimitiveTopology};
use bevy::prelude::*;

use super::marks::{MarkMaterial, MarkSpace};
use super::{OnScreen, reach_on_screen};
use crate::model_material::{ModelMaterial, selected_twin_of};
use crate::sight::Meetable;
use crate::view::WorldCamera;

const SELECTED_AMBER: Vec4 = Vec4::new(1.0, 0.6, 0.08, 1.0);
const CORNER_LINE_PX: f32 = 2.0;
const MIN_CORNER_ARM_PX: f32 = 10.0;
const MIN_BOX_SIDE_PX: f32 = 28.0;
const BOX_PADDING_PX: f32 = 4.0;
const CORNER_ARM_OF_SHORT_SIDE: f32 = 0.2;

/// The placements shown selected, by unique id: each tinted where it shows, and boxed by four
/// corners on the screen that read however far it stands, over whatever hides it.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection(pub BTreeSet<u32>);

#[derive(Component)]
pub(super) struct SelectedTwin;

#[derive(Component)]
pub(super) struct Tinted(Entity);

#[derive(Component)]
pub(super) struct Corners;

#[derive(Resource, Default)]
pub(super) struct SelectedLook {
    twins: HashMap<AssetId<ModelMaterial>, Handle<ModelMaterial>>,
    corners: Option<(Handle<Mesh>, Vec<[f32; 3]>)>,
}

impl SelectedLook {
    fn twin(
        &mut self,
        materials: &mut Assets<ModelMaterial>,
        drawn: AssetId<ModelMaterial>,
    ) -> Option<Handle<ModelMaterial>> {
        let slots = materials.get(drawn)?.extension.anim_slots;
        if let Some(handle) = self.twins.get(&drawn) {
            if materials
                .get(handle)
                .is_none_or(|m| m.extension.anim_slots != slots)
            {
                let twin = selected_twin_of(materials.get(drawn)?);
                materials.insert(handle.id(), twin).ok()?;
            }
            return Some(handle.clone());
        }
        let handle = materials.add(selected_twin_of(materials.get(drawn)?));
        self.twins.insert(drawn, handle.clone());
        Some(handle)
    }
}

type Batch<'a> = (
    Entity,
    &'a Meetable,
    &'a Mesh3d,
    &'a MeshTag,
    &'a MeshMaterial3d<ModelMaterial>,
    Option<&'a RenderLayers>,
    Option<&'a Tinted>,
);

type Twin<'a> = (
    &'a mut Mesh3d,
    &'a mut MeshTag,
    &'a mut MeshMaterial3d<ModelMaterial>,
    &'a mut RenderLayers,
);

pub(super) fn tint_selected(
    selection: Res<'_, Selection>,
    mut commands: Commands<'_, '_>,
    batches: Query<'_, '_, Batch<'_>, Without<SelectedTwin>>,
    tinted: Query<'_, '_, (), With<Tinted>>,
    mut twins: Query<'_, '_, Twin<'_>, With<SelectedTwin>>,
    mut look: ResMut<'_, SelectedLook>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
) {
    if selection.0.is_empty() && tinted.is_empty() {
        return;
    }
    for (batch, met, mesh, tag, drawn, layers, twin) in &batches {
        let wanted = met
            .seen
            .placement()
            .is_some_and(|id| selection.0.contains(&id));
        let layers = layers.cloned().unwrap_or_default();
        match (wanted, twin) {
            (true, None) => {
                let Some(material) = look.twin(&mut materials, drawn.id()) else {
                    continue;
                };
                let twin = commands
                    .spawn((
                        SelectedTwin,
                        mesh.clone(),
                        MeshTag(tag.0),
                        MeshMaterial3d(material),
                        layers,
                        ChildOf(batch),
                    ))
                    .id();
                commands.entity(batch).insert(Tinted(twin));
            }
            (true, Some(&Tinted(twin))) => {
                let Ok((mut m, mut t, mut mat, mut l)) = twins.get_mut(twin) else {
                    continue;
                };
                if m.0 != mesh.0 {
                    m.0 = mesh.0.clone();
                }
                if t.0 != tag.0 {
                    t.0 = tag.0;
                }
                if let Some(want) = look.twin(&mut materials, drawn.id())
                    && mat.0 != want
                {
                    mat.0 = want;
                }
                if *l != layers {
                    *l = layers;
                }
            }
            (false, Some(&Tinted(twin))) => {
                commands.entity(twin).try_despawn();
                commands.entity(batch).remove::<Tinted>();
            }
            (false, None) => {}
        }
    }
}

pub(super) fn spawn_corners(
    mut commands: Commands<'_, '_>,
    mut marks: ResMut<'_, Assets<MarkMaterial>>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut look: ResMut<'_, SelectedLook>,
) {
    let mesh = meshes.add(corners_mesh(Vec::new()));
    commands.spawn((
        Corners,
        Mesh3d(mesh.clone()),
        MeshMaterial3d(marks.add(MarkMaterial {
            colour: SELECTED_AMBER,
            space: MarkSpace::ScreenOnTop,
        })),
        NoFrustumCulling,
        Visibility::Hidden,
    ));
    look.corners = Some((mesh, Vec::new()));
}

type Placed<'a> = (&'a Meetable, &'a GlobalTransform, Option<&'a Aabb>);

#[allow(clippy::too_many_arguments)]
pub(super) fn corner_selected(
    selection: Res<'_, Selection>,
    camera: Query<'_, '_, (&Camera, &GlobalTransform), With<WorldCamera>>,
    batches: Query<'_, '_, Placed<'_>>,
    mut corners: Query<'_, '_, (&mut Transform, &mut Visibility), With<Corners>>,
    mut look: ResMut<'_, SelectedLook>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut on_screen: ResMut<'_, OnScreen>,
) {
    let (Ok((camera, eye)), Ok((mut placed, mut shown))) = (camera.single(), corners.single_mut())
    else {
        return;
    };
    let Some(size) = camera.logical_viewport_size() else {
        return;
    };
    let mut reach: BTreeMap<u32, Rect> = BTreeMap::new();
    if !selection.0.is_empty() {
        for (met, at, bound) in &batches {
            let (Some(id), Some(bound)) = (met.seen.placement(), bound) else {
                continue;
            };
            if selection.0.contains(&id)
                && let Some(r) = reach_on_screen((camera, eye), at, bound)
            {
                reach
                    .entry(id)
                    .and_modify(|all| *all = all.union(r))
                    .or_insert(r);
            }
        }
    }
    let mut boxes = BTreeMap::new();
    let mut lines: Vec<[f32; 3]> = Vec::new();
    for (id, r) in reach {
        if let Some(marks) = corner_marks(r, size) {
            boxes.insert(id, marks.box_px);
            lines.extend(marks.ndc);
        }
    }
    if on_screen.selected != boxes {
        on_screen.selected = boxes;
    }
    let want = if lines.is_empty() {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    if *shown != want {
        *shown = want;
    }
    let near = eye.translation();
    if placed.translation != near {
        placed.translation = near;
    }
    if let Some((mesh, drawn)) = look.corners.as_mut()
        && *drawn != lines
    {
        drawn.clone_from(&lines);
        meshes.insert(mesh.id(), corners_mesh(lines)).ok();
    }
}

fn corners_mesh(positions: Vec<[f32; 3]>) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
}

struct CornerMarks {
    box_px: Rect,
    ndc: Vec<[f32; 3]>,
}

fn corner_marks(reach_px: Rect, size: Vec2) -> Option<CornerMarks> {
    let side = reach_px.size().max(Vec2::splat(MIN_BOX_SIDE_PX));
    let r = Rect::from_center_size(reach_px.center(), side + 2.0 * BOX_PADDING_PX)
        .intersect(Rect::from_corners(Vec2::ZERO, size));
    if r.is_empty() {
        return None;
    }
    let arm = (CORNER_ARM_OF_SHORT_SIDE * r.size().min_element()).max(MIN_CORNER_ARM_PX);
    let ndc = |p: Vec2| [p.x / size.x * 2.0 - 1.0, 1.0 - p.y / size.y * 2.0, 0.0];
    let quad = |a: Vec2, b: Vec2| {
        let (lo, hi) = (a.min(b), a.max(b));
        let (p, q, s, t) = (lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y));
        [p, q, s, p, s, t].map(ndc)
    };
    let w = CORNER_LINE_PX;
    let mut out = Vec::with_capacity(48);
    for (corner, out_x, out_y) in [
        (r.min, 1.0, 1.0),
        (Vec2::new(r.max.x, r.min.y), -1.0, 1.0),
        (r.max, -1.0, -1.0),
        (Vec2::new(r.min.x, r.max.y), 1.0, -1.0),
    ] {
        let along_x = corner + Vec2::new(out_x * arm, out_y * w);
        let along_y = corner + Vec2::new(out_x * w, out_y * arm);
        out.extend(quad(corner, along_x));
        out.extend(quad(corner, along_y));
    }
    Some(CornerMarks {
        box_px: r,
        ndc: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_far_speck_is_boxed_at_the_least_side_and_a_box_stays_on_the_screen() {
        let size = Vec2::new(640.0, 360.0);
        let speck = Rect::from_center_size(Vec2::new(320.0, 180.0), Vec2::ONE);
        let marks = corner_marks(speck, size).expect("on the screen");
        assert_eq!(
            marks.ndc.len(),
            48,
            "four corners of two arms of two triangles"
        );
        let least = MIN_BOX_SIDE_PX + 2.0 * BOX_PADDING_PX;
        assert_eq!(marks.box_px.size(), Vec2::splat(least));
        let (lo, hi) = marks.ndc.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
            (lo.min(p[0]), hi.max(p[0]))
        });
        let across_px = (hi - lo) / 2.0 * size.x;
        assert!((across_px - least).abs() < 1e-3, "{across_px}");
        let past = corner_marks(Rect::new(600.0, 300.0, 900.0, 500.0), size).expect("partly on");
        assert!(
            past.ndc
                .iter()
                .all(|p| (-1.0..=1.0).contains(&p[0]) && (-1.0..=1.0).contains(&p[1]))
        );
        assert!(corner_marks(Rect::new(700.0, 400.0, 800.0, 500.0), size).is_none());
    }
}
