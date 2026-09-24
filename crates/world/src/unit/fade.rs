use bevy::camera::visibility::NoFrustumCulling;
use bevy::ecs::entity::EntityHashSet;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;
use model::ModelBlend;

use super::batch_anim::{UnitAlphaAnimated, UnitCards, unit_parts};
use super::body::BodyDressed;
use super::light::{LitBy, UnitLight};
use super::shade::UnitShade;
use crate::doodad_anim::MatAnim;
use crate::liquid::FarSide;
use crate::model_material::{BatchLook, GroundShade, ModelMaterial, ModelMaterials, Variant};
use crate::visibility::{translucent, with_alpha, with_interior_fog, with_shade_or_probe};

const APPEAR_SECS: f32 = 2.0;

/// A unit's render alpha, carried to all it draws: 1 opaque, 0 hidden.
#[derive(Component, Clone, Copy, Debug)]
pub struct UnitAlpha {
    pub alpha: f32,
    was_fading: bool,
}

/// A unit appearing: on its root from the frame its body first draws until it is opaque, two
/// seconds on. While it lasts it owns the unit's alpha, whatever the owner sets.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct UnitAppear {
    started: f32,
}

impl UnitAppear {
    pub(crate) fn at(now: f32) -> Self {
        Self { started: now }
    }

    /// The alpha at `now`: the cube of the share of the ramp run.
    pub fn alpha(self, now: f32) -> f32 {
        let t = ((now - self.started) / APPEAR_SECS).clamp(0.0, 1.0);
        t * t * t
    }

    fn done(self, now: f32) -> bool {
        now - self.started >= APPEAR_SECS
    }
}

impl Default for UnitAlpha {
    fn default() -> Self {
        Self {
            alpha: 1.0,
            was_fading: false,
        }
    }
}

#[derive(Component)]
pub(crate) struct PartMaterials {
    steady: Handle<ModelMaterial>,
    see_through: Handle<ModelMaterial>,
    probe_lit: Handle<ModelMaterial>,
    probe_lit_see_through: Handle<ModelMaterial>,
    depth_prime: Option<Handle<ModelMaterial>>,
}

impl PartMaterials {
    pub(crate) fn of(
        cache: &mut ModelMaterials,
        materials: &mut Assets<ModelMaterial>,
        look: &BatchLook,
        steady: Handle<ModelMaterial>,
        character: bool,
        light: &Buffer,
    ) -> Self {
        let multiplies = matches!(look.blend, ModelBlend::Mod | ModelBlend::Mod2x);
        let own_twin = !character && (multiplies || look.blend == ModelBlend::Blend);
        let see_through = if own_twin {
            steady.clone()
        } else {
            cache.get(materials, look, Variant::FadeTwin, light)
        };
        let probe_look = BatchLook {
            interior: true,
            shade: GroundShade::Lit,
            ..look.clone()
        };
        let probe_lit = cache.get(materials, &probe_look, Variant::Steady, light);
        let probe_lit_see_through = if own_twin {
            probe_lit.clone()
        } else {
            cache.get(materials, &probe_look, Variant::FadeTwin, light)
        };
        let primes = character || !(look.no_depth_write || look.no_depth_test || multiplies);
        let depth_prime = primes.then(|| cache.get(materials, look, Variant::DepthPrime, light));
        Self {
            steady,
            see_through,
            probe_lit,
            probe_lit_see_through,
            depth_prime,
        }
    }

    pub(crate) fn steady(&self) -> &Handle<ModelMaterial> {
        &self.steady
    }

    pub(crate) fn every_material(&self) -> [AssetId<ModelMaterial>; 4] {
        [
            &self.steady,
            &self.see_through,
            &self.probe_lit,
            &self.probe_lit_see_through,
        ]
        .map(Handle::id)
    }
}

#[derive(Component)]
pub(crate) struct PrimeTwin(Entity);

#[derive(Component)]
pub(crate) struct PrimeOf(Entity);

#[derive(Clone, Copy)]
enum Look {
    Steady,
    SeeThrough(f32),
    Hidden,
}

type Units<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static mut UnitAlpha>,
        Option<&'static UnitAppear>,
        Option<Ref<'static, UnitLight>>,
        Option<&'static UnitShade>,
        Option<&'static UnitCards>,
        Has<UnitAlphaAnimated>,
    ),
    With<BodyDressed>,
>;

type Parts<'w, 's> = Query<
    'w,
    's,
    (
        &'static PartMaterials,
        &'static mut MeshTag,
        &'static mut MeshMaterial3d<ModelMaterial>,
        &'static mut Visibility,
        Option<&'static MatAnim>,
    ),
>;

const OUTDOORS: UnitLight = UnitLight {
    lit_by: LitBy::Sky,
    room_fogged: false,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_unit_look(
    mut commands: Commands<'_, '_>,
    time: Res<'_, Time>,
    side: Res<'_, FarSide>,
    mut units: Units<'_, '_>,
    children: Query<'_, '_, &Children>,
    mut parts: Parts<'_, '_>,
    joined: Query<'_, '_, Entity, Added<PartMaterials>>,
    parents: Query<'_, '_, &ChildOf>,
    mut joined_units: Local<'_, EntityHashSet>,
) {
    joined_units.clear();
    for part in &joined {
        joined_units.extend(parents.iter_ancestors(part).filter(|&e| units.contains(e)));
    }
    let now = time.elapsed_secs();
    for (root, owner, appear, light, shade, cards, alpha_moves) in &mut units {
        let (look, fading) = if let Some(appear) = appear {
            if appear.done(now) {
                commands.entity(root).remove::<UnitAppear>();
                (Look::Steady, true)
            } else {
                (Look::SeeThrough(appear.alpha(now)), true)
            }
        } else if let Some(mut owner) = owner {
            let fading = owner.alpha < 1.0;
            let moving = fading || owner.was_fading;
            owner.was_fading = fading;
            let look = match owner.alpha {
                a if a >= 1.0 => Look::Steady,
                a if a <= 0.0 => Look::Hidden,
                a => Look::SeeThrough(a),
            };
            (look, moving)
        } else {
            (Look::Steady, false)
        };
        let relit = light.as_ref().is_some_and(Ref::is_changed);
        let joins = joined_units.contains(&root)
            || cards.is_some_and(|c| c.0.iter().any(|&e| joined.contains(e)));
        let parts_stale = fading || relit || joins || alpha_moves;
        if !parts_stale {
            continue;
        }
        let light = light.map_or(OUTDOORS, |l| *l);
        let shade_byte = shade.map(|s| u16::from(s.tag_byte()));
        for part in unit_parts(root, &children, cards) {
            if let Ok(item) = parts.get_mut(part) {
                show_part(part, item, look, light, shade_byte, &side);
            }
        }
    }
}

type PartItem<'a> = (
    &'a PartMaterials,
    Mut<'a, MeshTag>,
    Mut<'a, MeshMaterial3d<ModelMaterial>>,
    Mut<'a, Visibility>,
    Option<&'a MatAnim>,
);

fn show_part(
    part: Entity,
    (materials, mut tag, mut material, mut vis, authored): PartItem<'_>,
    look: Look,
    light: UnitLight,
    shade_byte: Option<u16>,
    side: &FarSide,
) {
    let probe = light.probe_lit();
    let authored = authored.map_or(1.0, |m| m.alpha);
    let (alpha, want) = match (look, probe) {
        (Look::Steady, false) => (1.0, &materials.steady),
        (Look::Steady, true) => (1.0, &materials.probe_lit),
        (Look::SeeThrough(alpha), false) => (alpha, &materials.see_through),
        (Look::SeeThrough(alpha), true) => (alpha, &materials.probe_lit_see_through),
        (Look::Hidden, _) => {
            vis.set_if_neq(Visibility::Hidden);
            return;
        }
    };
    if authored <= 0.0 {
        vis.set_if_neq(Visibility::Hidden);
        return;
    }
    vis.set_if_neq(Visibility::Inherited);
    let mut bits = with_alpha(tag.0, alpha * authored);
    bits = match (light.lit_by, shade_byte) {
        (LitBy::OwnProbe { slot }, _) => with_shade_or_probe(bits, slot),
        (_, Some(byte)) => with_shade_or_probe(bits, byte),
        (_, None) => bits,
    };
    bits = with_interior_fog(bits, light.room_fogged && light.lit_by != LitBy::Sky);
    if tag.0 != bits {
        tag.0 = bits;
    }
    let want = side.sided(part, want);
    if material.0 != *want {
        material.0 = want.clone();
    }
}

/// It runs after every tag writer and still in `Update`: a mesh spawned in `PostUpdate` can miss
/// Bevy's check for meshes whose material needs specializing, and the renderer then panics on it.
#[allow(clippy::type_complexity)]
pub(crate) fn sync_depth_primes(
    mut commands: Commands<'_, '_>,
    parts: Query<
        '_,
        '_,
        (
            Entity,
            &PartMaterials,
            &MeshTag,
            &Mesh3d,
            Option<&PrimeTwin>,
            Has<NoFrustumCulling>,
        ),
        Without<PrimeOf>,
    >,
    mut twins: Query<'_, '_, (&PrimeOf, &mut MeshTag, &mut Mesh3d), Without<PartMaterials>>,
) {
    for (part, fade, tag, mesh, twin, unculled) in &parts {
        let Some(material) = &fade.depth_prime else {
            continue;
        };
        let active = translucent(tag.0);
        match twin {
            None if active => {
                let mut t = commands.spawn((
                    Mesh3d(mesh.0.clone()),
                    MeshMaterial3d(material.clone()),
                    MeshTag(tag.0),
                    Transform::IDENTITY,
                    PrimeOf(part),
                    ChildOf(part),
                ));
                if unculled {
                    t.insert(NoFrustumCulling);
                }
                let t = t.id();
                commands.entity(part).insert(PrimeTwin(t));
            }
            Some(t) if !active => {
                commands.entity(t.0).despawn();
                commands.entity(part).remove::<PrimeTwin>();
            }
            _ => {}
        }
    }
    for (of, mut tag, mut mesh) in &mut twins {
        let Ok((_, _, part_tag, part_mesh, _, _)) = parts.get(of.0) else {
            continue;
        };
        if tag.0 != part_tag.0 {
            tag.0 = part_tag.0;
        }
        if mesh.0 != part_mesh.0 {
            mesh.0 = part_mesh.0.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::asset::uuid::Uuid;

    use super::*;
    use crate::visibility::alpha_bits;

    #[test]
    fn the_ramp_is_the_cube_of_its_share() {
        let appear = UnitAppear::at(10.0);
        assert!(appear.alpha(9.0).abs() < f32::EPSILON);
        assert!((appear.alpha(11.0) - 0.125).abs() < 1e-6);
        assert!((appear.alpha(12.0) - 1.0).abs() < f32::EPSILON);
        assert!((appear.alpha(20.0) - 1.0).abs() < f32::EPSILON);
    }

    fn material(n: u128) -> Handle<ModelMaterial> {
        Handle::Uuid(Uuid::from_u128(n), std::marker::PhantomData)
    }

    fn unit_with_a_part(app: &mut App, owner: Option<UnitAlpha>) -> (Entity, Entity) {
        let mut root = app.world_mut().spawn((
            UnitAppear::at(0.0),
            BodyDressed {
                parts: Vec::new(),
                slot: 0,
            },
        ));
        if let Some(owner) = owner {
            root.insert(owner);
        }
        let root = root.id();
        let part = app
            .world_mut()
            .spawn((
                PartMaterials {
                    steady: material(1),
                    see_through: material(2),
                    probe_lit: material(3),
                    probe_lit_see_through: material(4),
                    depth_prime: None,
                },
                MeshTag(alpha_bits(1.0)),
                MeshMaterial3d(material(1)),
                Visibility::Inherited,
                ChildOf(root),
            ))
            .id();
        (root, part)
    }

    fn step(app: &mut App, secs: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(secs));
        app.update();
    }

    fn look(app: &App, part: Entity) -> (u32, Handle<ModelMaterial>, Visibility) {
        let e = app.world().entity(part);
        (
            e.get::<MeshTag>().expect("a tag").0,
            e.get::<MeshMaterial3d<ModelMaterial>>()
                .expect("a material")
                .0
                .clone(),
            *e.get::<Visibility>().expect("a visibility"),
        )
    }

    #[test]
    fn a_unit_appears_see_through_then_settles_steady_whatever_its_owner_asks() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<FarSide>()
            .add_systems(Update, apply_unit_look);
        let owner = UnitAlpha {
            alpha: 0.0,
            was_fading: false,
        };
        let (root, part) = unit_with_a_part(&mut app, Some(owner));
        step(&mut app, 0.0);
        assert_eq!(
            look(&app, part),
            (alpha_bits(0.0), material(2), Visibility::Inherited),
            "its first frame draws faint, never hidden"
        );
        step(&mut app, 1.0);
        assert_eq!(look(&app, part).0, alpha_bits(0.125));
        step(&mut app, 1.0);
        assert_eq!(
            look(&app, part),
            (alpha_bits(1.0), material(1), Visibility::Inherited)
        );
        assert!(app.world().get::<UnitAppear>(root).is_none());
        step(&mut app, 0.1);
        assert_eq!(
            look(&app, part).2,
            Visibility::Hidden,
            "the owner's alpha takes over"
        );
    }
}
