use std::ops::Range;

use bevy::asset::AssetId;
use bevy::prelude::*;
use model::{ModelBlend, ParticleBlend};

/// One vertex of the effect stream: a world-space position (camera-relative in a draw that says
/// so), and the raw authored gamma-space colour, alpha the blend weight.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EffectVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EffectBlend {
    /// `(SRC_ALPHA, ONE)`, the shader premultiplying in gamma space.
    Add,
    Alpha,
    /// Blending off, depth write on.
    Opaque,
    /// [`Self::Opaque`] behind the alpha test at 224/255.
    AlphaKey,
    /// `dst · lerp(1, src, α)`.
    Multiply,
    /// `2 · src · dst`.
    Mod2x,
}

impl From<ParticleBlend> for EffectBlend {
    fn from(blend: ParticleBlend) -> Self {
        match blend {
            ParticleBlend::Add => EffectBlend::Add,
            ParticleBlend::Alpha => EffectBlend::Alpha,
            ParticleBlend::AlphaKey => EffectBlend::AlphaKey,
            ParticleBlend::Opaque => EffectBlend::Opaque,
        }
    }
}

impl EffectBlend {
    /// A model batch's blend on this lane. `additive` has to be passed beside the blend, which
    /// folds the additive modes into [`ModelBlend::Blend`]; alpha testing blends instead.
    pub fn from_model(blend: ModelBlend, additive: bool) -> Self {
        if additive {
            return EffectBlend::Add;
        }
        match blend {
            ModelBlend::Opaque => EffectBlend::Opaque,
            ModelBlend::AlphaTest | ModelBlend::Blend => EffectBlend::Alpha,
            ModelBlend::Mod => EffectBlend::Multiply,
            ModelBlend::Mod2x => EffectBlend::Mod2x,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectTopology {
    /// Four corners a quad, in perimeter order.
    Quads,
    Tris,
}

/// The colour a draw fogs toward: the client's per-blend fog table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EffectFog {
    Off,
    Scene,
    /// Black, so an added effect fades out rather than adding grey.
    Black,
    /// White, a multiply's identity.
    White,
    /// Grey, `2 · src · dst`'s identity.
    Grey,
}

impl EffectFog {
    /// An emitter's fog: its flag `0x8` off, black when it adds, else the scene's.
    pub fn for_blend(flags: u32, blend: ParticleBlend) -> Self {
        if flags & 0x8 != 0 {
            EffectFog::Off
        } else if matches!(blend, ParticleBlend::Add) {
            EffectFog::Black
        } else {
            EffectFog::Scene
        }
    }

    /// A model batch's fog policy (`0` scene, `1` black, `2` white, `3` grey, `4` off).
    pub fn from_model_policy(policy: u32) -> Self {
        match policy {
            1 => EffectFog::Black,
            2 => EffectFog::White,
            3 => EffectFog::Grey,
            4 => EffectFog::Off,
            _ => EffectFog::Scene,
        }
    }

    /// The draw's row in the lane's fog uniform.
    pub fn slot(self) -> u32 {
        match self {
            EffectFog::Off => 0,
            EffectFog::Scene => 1,
            EffectFog::Black => 2,
            EffectFog::White => 3,
            EffectFog::Grey => 4,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum EffectLighting {
    /// None: the colour burns as authored.
    #[default]
    None,
    /// The scene's ambient and sun on a normal pointing straight up, the one normal a particle
    /// quad carries.
    Scene,
}

/// One draw of the stream: a vertex range and what it is drawn with.
pub struct EffectDraw {
    /// The camera whose view draws it.
    pub cam: Entity,
    pub(crate) texture: AssetId<Image>,
    pub(crate) blend: EffectBlend,
    pub(crate) topology: EffectTopology,
    pub(crate) fog: EffectFog,
    pub lit: bool,
    pub(crate) sort_anchor: Vec3,
    pub(crate) sort_bias: f32,
    pub(crate) raster_bias: i32,
    pub(crate) raster_slope: f32,
    pub(crate) cam_relative: bool,
    pub(crate) no_depth_test: bool,
    pub range: Range<u32>,
    pub main_entity: Entity,
}

/// The frame's effect stream: every effect family's vertices, world-space or camera-relative as
/// their draw says, and the draws over them, cleared at the top of each frame's effect systems.
#[derive(Resource)]
pub struct EffectQuads {
    pub verts: Vec<EffectVertex>,
    pub draws: Vec<EffectDraw>,
    cleared_this_frame: bool,
}

impl Default for EffectQuads {
    fn default() -> Self {
        Self {
            verts: Vec::new(),
            draws: Vec::new(),
            cleared_this_frame: true,
        }
    }
}

/// Everything about one draw but its vertex range.
pub struct EffectDrawSpec {
    pub cam: Entity,
    pub texture: AssetId<Image>,
    pub blend: EffectBlend,
    pub fog: EffectFog,
    pub lighting: EffectLighting,
    /// The world point the draw sorts from.
    pub sort_anchor: Vec3,
    /// Added to the anchor's view depth when the transparent draws sort: positive draws later.
    pub sort_bias: f32,
    /// The rasterizer's constant depth bias. Non-zero, the draw is a ground-coplanar decal: its
    /// vertices stay absolute and go through the world meshes' own matrix.
    pub raster_bias: i32,
    /// The rasterizer's slope-scaled depth bias, for a coplanar draw seen at a grazing angle.
    pub raster_slope: f32,
    /// The vertices are already camera-relative, for geometry too small to survive absolute
    /// world coordinates in f32; the anchor stays absolute.
    pub cam_relative: bool,
    /// Draw over everything already in the depth buffer.
    pub no_depth_test: bool,
    pub main_entity: Entity,
}

impl EffectQuads {
    pub fn begin(&self) -> u32 {
        self.verts.len() as u32
    }

    /// Close a quad draw over everything pushed since `begin`; an empty one commits nothing.
    pub fn commit_quads(&mut self, start: u32, spec: EffectDrawSpec) {
        debug_assert_eq!((self.verts.len() as u32 - start) % 4, 0, "whole quads only");
        self.commit(start, EffectTopology::Quads, spec);
    }

    /// Close a triangle-list draw over everything pushed since `begin`.
    pub fn commit_tris(&mut self, start: u32, spec: EffectDrawSpec) {
        debug_assert_eq!(
            (self.verts.len() as u32 - start) % 3,
            0,
            "whole triangles only"
        );
        self.commit(start, EffectTopology::Tris, spec);
    }

    fn commit(&mut self, start: u32, topology: EffectTopology, spec: EffectDrawSpec) {
        debug_assert!(
            self.cleared_this_frame,
            "an effect written before `begin_effect_frame` is erased by it; order the writer after it"
        );
        let end = self.verts.len() as u32;
        if end > start {
            self.draws.push(EffectDraw {
                cam: spec.cam,
                texture: spec.texture,
                blend: spec.blend,
                topology,
                fog: spec.fog,
                lit: spec.lighting == EffectLighting::Scene,
                sort_anchor: spec.sort_anchor,
                sort_bias: spec.sort_bias,
                raster_bias: spec.raster_bias,
                raster_slope: spec.raster_slope,
                cam_relative: spec.cam_relative,
                no_depth_test: spec.no_depth_test,
                range: start..end,
                main_entity: spec.main_entity,
            });
        }
    }
}

/// Clears the stream; every writer runs after it.
pub fn begin_effect_frame(mut quads: ResMut<'_, EffectQuads>) {
    quads.verts.clear();
    quads.draws.clear();
    quads.cleared_this_frame = true;
}

pub(crate) fn clear_effect_frame_flag(mut quads: ResMut<'_, EffectQuads>) {
    quads.cleared_this_frame = false;
}

/// Draws batches of quads or triangles on the effect lane.
#[derive(bevy::ecs::system::SystemParam)]
pub struct WorldEffectDraw<'w> {
    quads: ResMut<'w, EffectQuads>,
}

impl WorldEffectDraw<'_> {
    /// Open a batch drawn through `cam` with `texture`: alpha-blended, unfogged and unlit until
    /// told otherwise. Nothing is drawn until it is closed.
    pub fn batch(&mut self, cam: Entity, texture: AssetId<Image>) -> EffectBatch<'_> {
        let start = self.quads.begin();
        EffectBatch {
            start,
            spec: EffectDrawSpec {
                cam,
                texture,
                blend: EffectBlend::Alpha,
                fog: EffectFog::Off,
                lighting: EffectLighting::None,
                sort_anchor: Vec3::ZERO,
                sort_bias: 0.0,
                raster_bias: 0,
                raster_slope: 0.0,
                cam_relative: false,
                no_depth_test: false,
                main_entity: Entity::PLACEHOLDER,
            },
            quads: &mut self.quads,
        }
    }
}

pub struct EffectBatch<'a> {
    quads: &'a mut EffectQuads,
    start: u32,
    spec: EffectDrawSpec,
}

impl EffectBatch<'_> {
    #[must_use]
    pub fn additive(mut self) -> Self {
        self.spec.blend = EffectBlend::Add;
        self
    }

    #[must_use]
    pub fn alpha(mut self) -> Self {
        self.spec.blend = EffectBlend::Alpha;
        self
    }

    #[must_use]
    pub fn multiply(mut self) -> Self {
        self.spec.blend = EffectBlend::Multiply;
        self
    }

    #[must_use]
    pub fn blend(mut self, blend: EffectBlend) -> Self {
        self.spec.blend = blend;
        self
    }

    #[must_use]
    pub fn fog(mut self, fog: EffectFog) -> Self {
        self.spec.fog = fog;
        self
    }

    /// The world point the batch sorts from.
    #[must_use]
    pub fn anchored(mut self, at: Vec3) -> Self {
        self.spec.sort_anchor = at;
        self
    }

    /// The sort bias and the rasterizer's constant depth bias.
    #[must_use]
    pub fn rung(mut self, sort: f32, raster: i32) -> Self {
        self.spec.sort_bias = sort;
        self.spec.raster_bias = raster;
        self
    }

    #[must_use]
    pub fn over_everything(mut self) -> Self {
        self.spec.no_depth_test = true;
        self
    }

    #[must_use]
    pub fn owner(mut self, entity: Entity) -> Self {
        self.spec.main_entity = entity;
        self
    }

    pub fn vertices(&mut self, verts: &[EffectVertex]) {
        self.quads.verts.extend_from_slice(verts);
    }

    pub fn extend(&mut self, verts: impl IntoIterator<Item = EffectVertex>) {
        self.quads.verts.extend(verts);
    }

    pub fn verts_mut(&mut self) -> &mut Vec<EffectVertex> {
        &mut self.quads.verts
    }

    pub fn tris(self) {
        self.quads.commit_tris(self.start, self.spec);
    }

    pub fn quads(self) {
        self.quads.commit_quads(self.start, self.spec);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn additive_wins_over_the_folded_blend() {
        for blend in [
            ModelBlend::Opaque,
            ModelBlend::AlphaTest,
            ModelBlend::Blend,
            ModelBlend::Mod,
            ModelBlend::Mod2x,
        ] {
            assert_eq!(EffectBlend::from_model(blend, true), EffectBlend::Add);
        }
        assert_eq!(
            EffectBlend::from_model(ModelBlend::AlphaTest, false),
            EffectBlend::Alpha
        );
        assert_eq!(
            EffectBlend::from_model(ModelBlend::Mod, false),
            EffectBlend::Multiply
        );
    }

    mod write_order {
        use super::*;

        #[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
        struct Place;

        fn before() {}

        fn writer(mut quads: ResMut<'_, EffectQuads>) {
            let start = quads.begin();
            quads.verts.extend(
                [EffectVertex {
                    pos: [0.0; 3],
                    uv: [0.0; 2],
                    color: [1.0; 4],
                }; 4],
            );
            quads.commit_quads(
                start,
                EffectDrawSpec {
                    cam: Entity::PLACEHOLDER,
                    texture: AssetId::default(),
                    blend: EffectBlend::Add,
                    fog: EffectFog::Off,
                    lighting: EffectLighting::None,
                    sort_anchor: Vec3::ZERO,
                    sort_bias: 0.0,
                    raster_bias: 0,
                    raster_slope: 0.0,
                    cam_relative: false,
                    no_depth_test: false,
                    main_entity: Entity::PLACEHOLDER,
                },
            );
        }

        fn run(edged: bool) -> App {
            let mut app = App::new();
            app.init_resource::<EffectQuads>();
            let w = writer.in_set(Place).after(before);
            app.add_systems(
                PostUpdate,
                if edged {
                    w.after(begin_effect_frame)
                } else {
                    w.before(begin_effect_frame)
                },
            );
            app.add_systems(PostUpdate, (before, begin_effect_frame).chain());
            app.add_systems(Last, clear_effect_frame_flag);
            app.update();
            app.update();
            app
        }

        #[test]
        fn a_writer_after_the_clear_keeps_its_draw() {
            let app = run(true);
            let quads = app.world().resource::<EffectQuads>();
            assert_eq!((quads.draws.len(), quads.verts.len()), (1, 4));
        }

        #[test]
        #[should_panic(expected = "erased by it")]
        fn a_writer_before_the_clear_trips_the_wire() {
            let _ = run(false);
        }
    }
}
