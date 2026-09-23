//! A unit drawn see-through: the alpha its owner sets, carried to every part it draws, and a
//! depth-only twin drawn ahead of each see-through part so the body blends as one layer rather
//! than darkening where it overlaps itself.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;
use model::ModelBlend;

use crate::model_material::{BatchLook, ModelMaterial, ModelMaterials, Variant};
use crate::visibility::{translucent, with_alpha};

/// A unit's render alpha, carried to all it draws: 1 opaque, 0 hidden.
#[derive(Component, Clone, Copy, Debug)]
pub struct UnitAlpha {
    pub alpha: f32,
    was_fading: bool,
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

pub(crate) fn apply_unit_alpha(
    mut units: Query<'_, '_, (Entity, &mut UnitAlpha)>,
    children: Query<'_, '_, &Children>,
    mut parts: Query<
        '_,
        '_,
        (
            &PartFade,
            &mut MeshTag,
            &mut MeshMaterial3d<ModelMaterial>,
            &mut Visibility,
        ),
    >,
) {
    for (root, mut unit) in &mut units {
        let fading = unit.alpha < 1.0;
        if !fading && !unit.was_fading {
            continue;
        }
        unit.was_fading = fading;
        let alpha = unit.alpha.clamp(0.0, 1.0);
        for part in children.iter_descendants(root) {
            let Ok((fade, mut tag, mut material, mut vis)) = parts.get_mut(part) else {
                continue;
            };
            let (visibility, alpha, want) = if alpha >= 1.0 {
                (Visibility::Inherited, 1.0, &fade.steady)
            } else if alpha <= 0.0 {
                vis.set_if_neq(Visibility::Hidden);
                continue;
            } else {
                (Visibility::Inherited, alpha, &fade.see_through)
            };
            vis.set_if_neq(visibility);
            let bits = with_alpha(tag.0, alpha);
            if tag.0 != bits {
                tag.0 = bits;
            }
            if material.0 != *want {
                material.0 = want.clone();
            }
        }
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
