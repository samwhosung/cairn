use std::io;

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::math::Vec3;
use bevy::reflect::TypePath;
use model::{M2Bounds, M2Light, parse_m2_bounds, parse_m2_lights, parse_m2_render_submeshes};

use crate::coords::wow_to_bevy;
use crate::model::ModelSubmesh;

/// An M2 as the world draws it: its render batches in skin order, its authored bounds and its
/// lights.
#[derive(Asset, TypePath)]
pub struct M2Model {
    pub submeshes: Vec<ModelSubmesh>,
    /// `None` when the header's bounds do not read.
    pub bounds: Option<M2Bounds>,
    pub lights: Vec<M2Light>,
}

impl M2Model {
    /// The bounding sphere a placement fades by: the header radius times the placement's scale,
    /// about the header box's centre (model space, Bevy axes). No bounds never fades.
    #[allow(
        clippy::manual_midpoint,
        reason = "the client's own sum-then-halve rounding"
    )]
    pub fn fade_sphere(&self, scale: f32) -> (f32, Vec3) {
        match &self.bounds {
            Some(b) => {
                let c = [
                    (b.bbox_min[0] + b.bbox_max[0]) * 0.5,
                    (b.bbox_min[1] + b.bbox_max[1]) * 0.5,
                    (b.bbox_min[2] + b.bbox_max[2]) * 0.5,
                ];
                (b.sphere_radius * scale, wow_to_bevy(c))
            }
            None => (f32::INFINITY, Vec3::ZERO),
        }
    }
}

#[derive(Default, TypePath)]
pub(crate) struct M2Loader;

impl AssetLoader for M2Loader {
    type Asset = M2Model;
    type Settings = ();
    type Error = io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        ctx: &mut LoadContext<'_>,
    ) -> Result<M2Model, io::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let subs = parse_m2_render_submeshes(&bytes, "", &[]).map_err(io::Error::other)?;
        let submeshes = subs
            .into_iter()
            .map(|sub| ModelSubmesh::load(ctx, sub))
            .collect();
        Ok(M2Model {
            submeshes,
            bounds: parse_m2_bounds(&bytes).ok(),
            lights: parse_m2_lights(&bytes),
        })
    }

    fn extensions(&self) -> &[&str] {
        &["m2"]
    }
}
