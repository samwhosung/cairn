use std::sync::Arc;

use bevy::asset::{LoadContext, RenderAssetUsages};
use bevy::camera::primitives::Aabb;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use model::{BillboardKind, RenderSubmesh};

use crate::coords::wow_to_bevy;
use crate::source::{Repeat, texture_url};

/// One render batch of a loaded model: its geometry and its texture.
#[derive(Clone)]
pub struct ModelSubmesh {
    /// The batch as parsed, in the model's own space (WoW axes).
    pub geometry: Arc<RenderSubmesh>,
    /// The mesh's bound, in the mesh's own (Bevy) space; `None` for a batch with no vertices.
    pub aabb: Option<Aabb>,
    /// `None` without a texture, which the client draws as white.
    pub texture: Option<Handle<Image>>,
    pub billboard: Option<BillboardInfo>,
}

/// A batch riding an M2 billboard bone: the mesh is built about `pivot` (model space, Bevy axes)
/// so it can be turned to the camera there.
#[derive(Clone, Copy, Debug)]
pub struct BillboardInfo {
    pub pivot: Vec3,
    pub kind: BillboardKind,
}

impl ModelSubmesh {
    pub(crate) fn load(ctx: &mut LoadContext<'_>, sub: RenderSubmesh) -> Self {
        let aabb = Aabb::enclosing(mesh_positions(&sub).into_iter().map(Vec3::from));
        let repeat = Repeat {
            u: sub.wrap_x,
            v: sub.wrap_y,
        };
        let texture = sub
            .texture
            .as_deref()
            .map(|t| ctx.load::<Image>(texture_url(t, repeat)));
        let billboard = sub.billboard.as_ref().map(|b| BillboardInfo {
            pivot: wow_to_bevy(b.pivot),
            kind: b.kind,
        });
        Self {
            geometry: Arc::new(sub),
            aabb,
            texture,
            billboard,
        }
    }
}

fn mesh_positions(sub: &RenderSubmesh) -> Vec<[f32; 3]> {
    let center = sub
        .billboard
        .as_ref()
        .map_or(Vec3::ZERO, |b| wow_to_bevy(b.pivot));
    sub.positions
        .iter()
        .map(|p| (wow_to_bevy(*p) - center).to_array())
        .collect()
}

/// A batch's mesh in Bevy axes, with its authored normals — turned round on a billboard card
/// authored facing away from the viewer — or smooth ones when it has none.
pub(crate) fn submesh_mesh(sub: &RenderSubmesh) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, mesh_positions(sub));
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, sub.uvs.clone());
    mesh.insert_indices(Indices::U32(sub.indices.clone()));
    if sub.vertex_colors.len() == sub.positions.len() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, sub.vertex_colors.clone());
    }
    if sub.normals.len() == sub.positions.len() {
        let flip = sub.billboard_card_faces_away();
        let normals: Vec<[f32; 3]> = sub
            .normals
            .iter()
            .map(|n| {
                let b = wow_to_bevy(*n);
                if flip { -b } else { b }.to_array()
            })
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    } else {
        mesh.compute_normals();
    }
    mesh
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use bevy::camera::primitives::MeshAabb;
    use bevy::mesh::VertexAttributeValues;
    use model::Billboard;

    use super::*;

    fn quad(normal: [f32; 3]) -> RenderSubmesh {
        RenderSubmesh {
            positions: vec![
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.0, 1.0],
            ],
            normals: vec![normal; 4],
            uvs: vec![[0.0, 0.0]; 4],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..RenderSubmesh::default()
        }
    }

    fn floats(mesh: &Mesh, attribute: bevy::mesh::MeshVertexAttribute) -> Vec<[f32; 3]> {
        match mesh.attribute(attribute) {
            Some(VertexAttributeValues::Float32x3(v)) => v.clone(),
            _ => panic!("no float3 attribute"),
        }
    }

    #[test]
    fn a_card_is_built_about_its_pivot_and_faces_the_viewer() {
        let mut card = quad([-1.0, 0.0, 0.0]);
        card.billboard = Some(Billboard {
            pivot: [1.0, 0.5, 0.5],
            bone: 0,
            kind: BillboardKind::Spherical,
            scale_anim: None,
            seq_translations: Vec::new(),
        });
        let mesh = submesh_mesh(&card);
        let first = floats(&mesh, Mesh::ATTRIBUTE_POSITION)[0];
        let expected = wow_to_bevy([1.0, 0.0, 0.0]) - wow_to_bevy([1.0, 0.5, 0.5]);
        assert!((Vec3::from(first) - expected).length() < 1e-6);
        let normal = floats(&mesh, Mesh::ATTRIBUTE_NORMAL)[0];
        assert!((Vec3::from(normal) - wow_to_bevy([1.0, 0.0, 0.0])).length() < 1e-6);
    }

    #[test]
    fn an_ordinary_batch_keeps_its_place_and_normals() {
        let sub = quad([-1.0, 0.0, 0.0]);
        let mesh = submesh_mesh(&sub);
        let positions = floats(&mesh, Mesh::ATTRIBUTE_POSITION);
        assert_eq!(positions[2], wow_to_bevy([1.0, 1.0, 1.0]).to_array());
        let normal = floats(&mesh, Mesh::ATTRIBUTE_NORMAL)[0];
        assert_eq!(normal, wow_to_bevy([-1.0, 0.0, 0.0]).to_array());
        assert!(mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_none());
        let bound = Aabb::enclosing(mesh_positions(&sub).into_iter().map(Vec3::from));
        assert_eq!(bound, mesh.compute_aabb());
    }
}
