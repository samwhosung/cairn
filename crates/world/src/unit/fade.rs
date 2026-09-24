//! A unit drawn see-through: the ramp it appears on, the alpha its owner sets, carried to every
//! part it draws, and a depth-only twin drawn ahead of each see-through part so the body blends as
//! one layer rather than darkening where it overlaps itself.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;
use model::ModelBlend;

use crate::liquid::FarSide;
use crate::model_material::{BatchLook, ModelMaterial, ModelMaterials, Variant};
use crate::visibility::{translucent, with_alpha};

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
pub(crate) struct PartFade {
    steady: Handle<ModelMaterial>,
    see_through: Handle<ModelMaterial>,
    depth_prime: Option<Handle<ModelMaterial>>,
}

impl PartFade {
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
        let primes = character || !(look.no_depth_write || look.no_depth_test || multiplies);
        let depth_prime = primes.then(|| cache.get(materials, look, Variant::DepthPrime, light));
        Self {
            steady,
            see_through,
            depth_prime,
        }
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
    ),
    Or<(With<UnitAlpha>, With<UnitAppear>)>,
>;

type Parts<'w, 's> = Query<
    'w,
    's,
    (
        &'static PartFade,
        &'static mut MeshTag,
        &'static mut MeshMaterial3d<ModelMaterial>,
        &'static mut Visibility,
    ),
>;

/// The appear ramp never hides a part: at its start it draws at the smallest alpha there is.
pub(crate) fn apply_unit_alpha(
    mut commands: Commands<'_, '_>,
    time: Res<'_, Time>,
    side: Res<'_, FarSide>,
    mut units: Units<'_, '_>,
    children: Query<'_, '_, &Children>,
    mut parts: Parts<'_, '_>,
) {
    let now = time.elapsed_secs();
    for (root, owner, appear) in &mut units {
        let look = if let Some(appear) = appear {
            if appear.done(now) {
                commands.entity(root).remove::<UnitAppear>();
                Look::Steady
            } else {
                Look::SeeThrough(appear.alpha(now))
            }
        } else {
            let Some(mut owner) = owner else {
                continue;
            };
            let fading = owner.alpha < 1.0;
            if !fading && !owner.was_fading {
                continue;
            }
            owner.was_fading = fading;
            match owner.alpha {
                a if a >= 1.0 => Look::Steady,
                a if a <= 0.0 => Look::Hidden,
                a => Look::SeeThrough(a),
            }
        };
        for part in children.iter_descendants(root) {
            if let Ok(item) = parts.get_mut(part) {
                show_part(part, item, look, &side);
            }
        }
    }
}

fn show_part(
    part: Entity,
    (fade, mut tag, mut material, mut vis): (
        &PartFade,
        Mut<'_, MeshTag>,
        Mut<'_, MeshMaterial3d<ModelMaterial>>,
        Mut<'_, Visibility>,
    ),
    look: Look,
    side: &FarSide,
) {
    let (alpha, want) = match look {
        Look::Steady => (1.0, &fade.steady),
        Look::SeeThrough(alpha) => (alpha, &fade.see_through),
        Look::Hidden => {
            vis.set_if_neq(Visibility::Hidden);
            return;
        }
    };
    vis.set_if_neq(Visibility::Inherited);
    let bits = with_alpha(tag.0, alpha);
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
            &PartFade,
            &MeshTag,
            &Mesh3d,
            Option<&PrimeTwin>,
            Has<NoFrustumCulling>,
        ),
        Without<PrimeOf>,
    >,
    mut twins: Query<'_, '_, (&PrimeOf, &mut MeshTag, &mut Mesh3d), Without<PartFade>>,
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
        let mut root = app.world_mut().spawn(UnitAppear::at(0.0));
        if let Some(owner) = owner {
            root.insert(owner);
        }
        let root = root.id();
        let part = app
            .world_mut()
            .spawn((
                PartFade {
                    steady: material(1),
                    see_through: material(2),
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
            .add_systems(Update, apply_unit_alpha);
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
