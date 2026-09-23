use std::sync::Arc;

use bevy::asset::LoadContext;
use bevy::camera::primitives::Aabb;
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
